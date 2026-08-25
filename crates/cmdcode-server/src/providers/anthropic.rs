//! Native Anthropic Messages API upstream adapter.
//!
//! Converts the internal OpenAI-format request into Anthropic's
//! `/v1/messages` wire format and translates Anthropic SSE events back into
//! the OpenAI-format chunks every downstream renderer consumes.
//!
//! Configure with `type: "anthropic"` in providers.json:
//!
//! ```json
//! {"providers": {"claude-direct": {
//!     "type": "anthropic",
//!     "options": {"apiKey": "{env:ANTHROPIC_API_KEY}"}
//! }}}
//! ```

use super::{Provider, RequestContext};
use crate::upstream::{LineOutcome, StreamState};
use cmdcode_core::auth::AuthManager;
use cmdcode_core::error::UpstreamError;
use cmdcode_core::wire_format::{ChatCompletionRequest, OpenAiMessage};
use serde_json::{json, Value};

const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Anthropic requires an explicit cap; generous default when unset.
const DEFAULT_MAX_TOKENS: u32 = 32_000;

/// Native Anthropic provider.
#[derive(Clone)]
pub struct AnthropicProvider {
    /// Upstream base URL (default `https://api.anthropic.com`).
    pub base_url: String,
    /// API key (`sk-ant-...`).
    pub api_key: Option<String>,
}

// --- Request conversion (OpenAI internal -> Anthropic) ---------------------

fn text_of(content: &Option<Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// Convert internal messages to Anthropic `messages` + `system`.
fn convert_messages(messages: &[OpenAiMessage]) -> (String, Vec<Value>) {
    let mut system_parts: Vec<String> = Vec::new();
    let mut out: Vec<Value> = Vec::new();

    for msg in messages {
        match msg.role.as_str() {
            "system" => system_parts.push(text_of(&msg.content)),
            "user" => out.push(json!({
                "role": "user",
                "content": msg.content.clone().unwrap_or_else(|| json!("")),
            })),
            "assistant" => {
                let mut blocks: Vec<Value> = Vec::new();
                if let Some(text) = msg
                    .content
                    .as_ref()
                    .and_then(|c| c.as_str())
                    .filter(|s| !s.is_empty())
                {
                    blocks.push(json!({"type": "text", "text": text}));
                }
                if let Some(calls) = &msg.tool_calls {
                    for call in calls {
                        let args_str = call
                            .function
                            .as_ref()
                            .and_then(|f| f.arguments.clone())
                            .unwrap_or_else(|| "{}".into());
                        blocks.push(json!({
                            "type": "tool_use",
                            "id": call.id.clone().unwrap_or_else(|| "call_0".into()),
                            "name": call.function.as_ref().and_then(|f| f.name.clone()).unwrap_or_default(),
                            "input": serde_json::from_str::<Value>(&args_str)
                                .unwrap_or_else(|_| json!({})),
                        }));
                    }
                }
                if blocks.is_empty() {
                    blocks.push(json!({"type": "text", "text": ""}));
                }
                out.push(json!({"role": "assistant", "content": blocks}));
            }
            "tool" => {
                // Anthropic requires tool_result inside the next user turn;
                // consecutive tool messages merge into one user message.
                let block = json!({
                    "type": "tool_result",
                    "tool_use_id": msg.tool_call_id.clone().unwrap_or_default(),
                    "content": text_of(&msg.content),
                });
                match out.last_mut() {
                    // Extend a preceding tool-result user turn.
                    Some(last)
                        if last["role"] == "user"
                            && last["content"]
                                .as_array()
                                .map(|a| {
                                    a.first()
                                        .and_then(|b| b.get("type"))
                                        .and_then(|t| t.as_str())
                                        == Some("tool_result")
                                })
                                .unwrap_or(false) =>
                    {
                        if let Some(arr) = last["content"].as_array_mut() {
                            arr.push(block);
                        }
                    }
                    _ => out.push(json!({"role": "user", "content": [block]})),
                }
            }
            _ => {}
        }
    }

    (system_parts.join("\n\n"), out)
}

impl AnthropicProvider {
    /// Build the `/v1/messages` request body from the normalized context.
    fn make_body(&self, ctx: &RequestContext<'_>) -> Value {
        let b: &ChatCompletionRequest = ctx.body;
        let (mut system, messages) = convert_messages(&b.messages);
        if let Some(taste) = &ctx.taste_section {
            system = if system.is_empty() {
                taste.clone()
            } else {
                format!("{taste}\n\n{system}")
            };
        }

        let mut body = json!({
            "model": ctx.model.as_str(),
            "max_tokens": b.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
            "messages": messages,
            "stream": true,
        });
        if !system.is_empty() {
            body["system"] = json!(system);
        }
        if let Some(t) = b.temperature {
            body["temperature"] = json!(t);
        }
        if let Some(p) = b.top_p {
            body["top_p"] = json!(p);
        }
        if let Some(stops) = &b.stop {
            body["stop_sequences"] = json!(stops);
        }
        if let Some(effort) = ctx.effort {
            // Map reasoning effort onto extended thinking budget.
            let budget = match effort.as_str() {
                "low" => 2048,
                "high" | "xhigh" => 16_384,
                _ => 8192,
            };
            body["thinking"] = json!({"type": "enabled", "budget_tokens": budget});
        }

        // Tools.
        let tools = convert_tools(b.tools.as_ref());
        if let Some(t) = tools {
            body["tools"] = t;
        }
        body
    }
}

fn convert_tools(tools: Option<&Vec<cmdcode_core::wire_format::OpenAiTool>>) -> Option<Value> {
    let tools = tools?;
    let decls: Vec<Value> = tools
        .iter()
        .filter_map(|t| t.function.as_ref())
        .map(|f| {
            json!({
                "name": f.name,
                "description": f.description.clone().unwrap_or_default(),
                "input_schema": f.parameters.clone().unwrap_or_else(|| json!({"type":"object"})),
            })
        })
        .collect();
    if decls.is_empty() {
        None
    } else {
        Some(json!(decls))
    }
}

// --- Response conversion ---------------------------------------------------

/// Map an Anthropic stop reason to the OpenAI finish reason.
fn finish_from_anthropic(reason: &str) -> &'static str {
    match reason {
        "tool_use" => "tool_calls",
        "max_tokens" => "length",
        "refusal" => "content_filter",
        _ => "stop",
    }
}

/// Convert a complete Anthropic message JSON into an OpenAI completion.
pub fn anthropic_to_completion(resp: &Value, model: &str) -> Value {
    let mut text_parts: Vec<&str> = Vec::new();
    let mut reasoning_parts: Vec<&str> = Vec::new();
    let mut tool_calls: Vec<Value> = Vec::new();

    let Some(blocks) = resp.get("content").and_then(|c| c.as_array()) else {
        return json!({});
    };
    for block in blocks {
        match block.get("type").and_then(|t| t.as_str()) {
            Some("text") => {
                if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                    text_parts.push(t);
                }
            }
            Some("thinking") => {
                if let Some(t) = block.get("thinking").and_then(|t| t.as_str()) {
                    reasoning_parts.push(t);
                }
            }
            Some("tool_use") => {
                tool_calls.push(json!({
                    "id": block.get("id").cloned().unwrap_or_else(|| json!("call_0")),
                    "type": "function",
                    "function": {
                        "name": block.get("name").cloned().unwrap_or_else(|| json!("")),
                        "arguments": serde_json::to_string(
                            &block.get("input").cloned().unwrap_or_else(|| json!({}))
                        ).unwrap_or_default(),
                    },
                }));
            }
            _ => {}
        }
    }

    let stop_reason = resp
        .get("stop_reason")
        .and_then(|s| s.as_str())
        .unwrap_or("end_turn");
    let input = resp
        .pointer("/usage/input_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let output = resp
        .pointer("/usage/output_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    let mut completion = json!({
        "id": resp.get("id").cloned().unwrap_or_else(|| json!("chatcmpl-anthropic")),
        "object": "chat.completion",
        "created": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        "model": model,
        "choices": [{
            "index": 0,
            "finish_reason": finish_from_anthropic(stop_reason),
            "message": {
                "role": "assistant",
                "content": text_parts.join(""),
                "reasoning_content": if reasoning_parts.is_empty() {
                    Value::Null
                } else {
                    json!(reasoning_parts.join(""))
                },
                "tool_calls": if tool_calls.is_empty() { Value::Null } else { json!(tool_calls) },
            },
        }],
        "usage": {
            "prompt_tokens": input,
            "completion_tokens": output,
            "total_tokens": input + output,
        },
    });
    if tool_calls.is_empty() {
        completion["choices"][0]["message"]["tool_calls"] = Value::Null;
    } else {
        completion["choices"][0]["message"]["tool_calls"] = json!(tool_calls);
    }
    completion
}

#[async_trait::async_trait]
impl Provider for AnthropicProvider {
    fn name(&self) -> &'static str {
        "anthropic"
    }

    fn endpoint(&self, _model: &str, _streaming: bool) -> String {
        format!("{}/v1/messages", self.base_url.trim_end_matches('/'))
    }

    async fn headers(
        &self,
        _auth: &AuthManager,
        _cwd: &str,
    ) -> Result<Vec<(String, String)>, UpstreamError> {
        Ok(vec![
            ("Content-Type".into(), "application/json".into()),
            ("x-api-key".into(), self.api_key.clone().unwrap_or_default()),
            ("anthropic-version".into(), ANTHROPIC_VERSION.into()),
        ])
    }

    fn build_body(&self, ctx: &RequestContext<'_>) -> Value {
        self.make_body(ctx)
    }

    fn translate_line<'a>(&self, line: &str, state: &mut StreamState<'a>) -> LineOutcome {
        anthropic_translate_line(line, state)
    }

    fn parse_non_streaming(&self, text: &str, model: &str) -> Result<Value, UpstreamError> {
        let parsed: Value = serde_json::from_str(text).map_err(|e| UpstreamError::HttpError {
            status: 502,
            body: format!("invalid anthropic response: {e}"),
        })?;
        if parsed.get("type").and_then(|t| t.as_str()) == Some("error") {
            return Err(UpstreamError::HttpError {
                status: 502,
                body: parsed.to_string(),
            });
        }
        Ok(anthropic_to_completion(&parsed, model))
    }

    fn is_auth_rejected(&self, status: u16) -> bool {
        status == 401 || status == 403
    }
}

/// Translate one line of Anthropic SSE into OpenAI-format chunk frames.
///
/// Only `data:` lines carry JSON; `event:` lines are skipped (the type field
/// inside the data payload is authoritative).
pub fn anthropic_translate_line<'a>(line: &str, state: &mut StreamState<'a>) -> LineOutcome {
    let trimmed = line.trim();
    if trimmed.is_empty() || !trimmed.starts_with("data:") {
        return LineOutcome::Skip;
    }
    let data = trimmed[5..].trim();
    if data.is_empty() {
        return LineOutcome::Skip;
    }
    let Ok(ev) = serde_json::from_str::<Value>(data) else {
        return LineOutcome::Skip;
    };

    match ev.get("type").and_then(|t| t.as_str()).unwrap_or("") {
        "content_block_start" => {
            let index = ev.get("index").and_then(|i| i.as_u64()).unwrap_or(0);
            let block = ev
                .get("content_block")
                .cloned()
                .unwrap_or_else(|| json!({}));
            if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                let frame = json!({
                    "id": state.completion_id,
                    "object": "chat.completion.chunk",
                    "model": state.model,
                    "choices": [{
                        "index": 0,
                        "delta": {"tool_calls": [{
                            "index": index,
                            "id": block.get("id").cloned().unwrap_or_else(|| json!("call_0")),
                            "type": "function",
                            "function": {
                                "name": block.get("name").cloned().unwrap_or_else(|| json!("")),
                                "arguments": "",
                            },
                        }]},
                        "finish_reason": null,
                    }],
                });
                return LineOutcome::Emit(format!(
                    "data: {}\n\n",
                    serde_json::to_string(&frame).unwrap_or_default()
                ));
            }
            LineOutcome::Skip
        }
        "content_block_delta" => {
            let delta = ev.get("delta").cloned().unwrap_or_else(|| json!({}));
            let frame = match delta.get("type").and_then(|t| t.as_str()) {
                Some("text_delta") => {
                    let text = delta.get("text").cloned().unwrap_or_else(|| json!(""));
                    json!({
                        "id": state.completion_id,
                        "object": "chat.completion.chunk",
                        "model": state.model,
                        "choices": [{"index": 0, "delta": {"content": text}, "finish_reason": null}],
                    })
                }
                Some("thinking_delta") => {
                    let thinking = delta.get("thinking").cloned().unwrap_or_else(|| json!(""));
                    json!({
                        "id": state.completion_id,
                        "object": "chat.completion.chunk",
                        "model": state.model,
                        "choices": [{"index": 0, "delta": {"reasoning_content": thinking}, "finish_reason": null}],
                    })
                }
                Some("input_json_delta") => {
                    let partial = delta
                        .get("partial_json")
                        .cloned()
                        .unwrap_or_else(|| json!(""));
                    let index = ev.get("index").and_then(|i| i.as_u64()).unwrap_or(0);
                    json!({
                        "id": state.completion_id,
                        "object": "chat.completion.chunk",
                        "model": state.model,
                        "choices": [{
                            "index": 0,
                            "delta": {"tool_calls": [{
                                "index": index,
                                "function": {"arguments": partial},
                            }]},
                            "finish_reason": null,
                        }],
                    })
                }
                _ => return LineOutcome::Skip,
            };
            LineOutcome::Emit(format!(
                "data: {}\n\n",
                serde_json::to_string(&frame).unwrap_or_default()
            ))
        }
        "message_delta" => {
            let stop = ev
                .pointer("/delta/stop_reason")
                .and_then(|s| s.as_str())
                .map(finish_from_anthropic)
                .unwrap_or("stop");
            let usage_out = ev
                .pointer("/usage/output_tokens")
                .cloned()
                .unwrap_or_else(|| json!(0));
            let frame = json!({
                "id": state.completion_id,
                "object": "chat.completion.chunk",
                "model": state.model,
                "choices": [{"index": 0, "delta": {}, "finish_reason": stop}],
                "usage": {"prompt_tokens": 0, "completion_tokens": usage_out},
            });
            LineOutcome::Emit(format!(
                "data: {}\n\n",
                serde_json::to_string(&frame).unwrap_or_default()
            ))
        }
        "message_stop" => LineOutcome::EmitAndStop("data: [DONE]\n\n".into()),
        _ => LineOutcome::Skip,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::providers::RequestContext;
    use cmdcode_core::types::ModelId;
    use cmdcode_core::wire_format::ChatCompletionRequest;

    fn ctx<'a>(body: &'a ChatCompletionRequest, model: &'a ModelId) -> RequestContext<'a> {
        RequestContext {
            model,
            body,
            effort: None,
            cwd: "/tmp",
            taste_section: None,
        }
    }

    #[test]
    fn test_build_body_system_tools_and_tool_results() {
        let body: ChatCompletionRequest = serde_json::from_value(json!({
            "model": "x",
            "messages": [
                {"role": "system", "content": "Be terse."},
                {"role": "user", "content": "Weather in Paris?"},
                {"role": "assistant", "content": null, "tool_calls": [
                    {"id": "call_1", "type": "function",
                     "function": {"name": "get_weather", "arguments": "{\"city\":\"Paris\"}"}}
                ]},
                {"role": "tool", "tool_call_id": "call_1", "content": "22C"}
            ],
            "tools": [{"type": "function", "function": {
                "name": "get_weather", "parameters": {"type": "object"}}}],
            "max_tokens": 512
        }))
        .unwrap();
        let provider = AnthropicProvider {
            base_url: "https://api.anthropic.com".into(),
            api_key: Some("k".into()),
        };
        let model = ModelId::new("claude-sonnet-5");
        let out = provider.make_body(&ctx(&body, &model));
        assert_eq!(out["model"], "claude-sonnet-5");
        assert_eq!(out["max_tokens"], 512);
        assert_eq!(out["system"], "Be terse.");
        // System is extracted -> [0]=user, [1]=assistant(tool_use), [2]=user(tool_result)
        assert_eq!(out["messages"][1]["content"][0]["type"], "tool_use");
        let last = &out["messages"][2];
        assert_eq!(last["role"], "user");
        assert_eq!(last["content"][0]["type"], "tool_result");
        assert_eq!(last["content"][0]["tool_use_id"], "call_1");
        // tools flat
        assert_eq!(out["tools"][0]["name"], "get_weather");
    }

    #[test]
    fn test_translate_line_event_sequence() {
        let mut state = StreamState {
            completion_id: "chatcmpl-t",
            created: 0,
            model: "claude-sonnet-5",
            tool_index: 0,
            skipped: 0,
            finish_seen: false,
            tool_parts: std::collections::HashMap::new(),
            skipped_by_type: std::collections::HashMap::new(),
        };

        // Text delta.
        let line = r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}"#;
        match super::anthropic_translate_line(line, &mut state) {
            LineOutcome::Emit(frame) => {
                let parsed: Value =
                    serde_json::from_str(frame.trim_start_matches("data: ").trim()).unwrap();
                assert_eq!(
                    parsed.pointer("/choices/0/delta/content"),
                    Some(&json!("Hi"))
                );
            }
            _ => panic!("expected Emit"),
        }

        // Thinking delta.
        let line = r#"data: {"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"hmm"}}"#;
        match super::anthropic_translate_line(line, &mut state) {
            LineOutcome::Emit(frame) => {
                let parsed: Value =
                    serde_json::from_str(frame.trim_start_matches("data: ").trim()).unwrap();
                assert!(parsed
                    .pointer("/choices/0/delta/reasoning_content")
                    .is_some());
            }
            _ => panic!("expected Emit"),
        }

        // Tool call start + json delta.
        let start = r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"call_9","name":"f"}}"#;
        match super::anthropic_translate_line(start, &mut state) {
            LineOutcome::Emit(frame) => {
                let parsed: Value =
                    serde_json::from_str(frame.trim_start_matches("data: ").trim()).unwrap();
                assert_eq!(
                    parsed.pointer("/choices/0/delta/tool_calls/0/function/name"),
                    Some(&json!("f"))
                );
            }
            _ => panic!("expected Emit"),
        }
        let delta = r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"x\":1}"}}"#;
        match super::anthropic_translate_line(delta, &mut state) {
            LineOutcome::Emit(_) => {}
            _ => panic!("expected Emit"),
        }

        // Finish.
        let fin = r#"data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":7}}"#;
        match super::anthropic_translate_line(fin, &mut state) {
            LineOutcome::Emit(frame) => {
                let parsed: Value =
                    serde_json::from_str(frame.trim_start_matches("data: ").trim()).unwrap();
                assert_eq!(
                    parsed.pointer("/choices/0/finish_reason"),
                    Some(&json!("tool_calls"))
                );
            }
            _ => panic!("expected Emit"),
        }
        let stop = r#"data: {"type":"message_stop"}"#;
        assert!(matches!(
            super::anthropic_translate_line(stop, &mut state),
            LineOutcome::EmitAndStop(_)
        ));
    }

    #[test]
    fn test_parse_non_streaming() {
        let provider = AnthropicProvider {
            base_url: String::new(),
            api_key: None,
        };
        let resp = json!({
            "id": "msg_1",
            "content": [
                {"type": "thinking", "thinking": "let me think"},
                {"type": "text", "text": "Answer"},
                {"type": "tool_use", "id": "call_3", "name": "f", "input": {"a": 1}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 4, "output_tokens": 6}
        });
        let out = provider
            .parse_non_streaming(&serde_json::to_string(&resp).unwrap(), "claude")
            .unwrap();
        assert_eq!(
            out.pointer("/choices/0/finish_reason"),
            Some(&json!("tool_calls"))
        );
        assert_eq!(
            out.pointer("/choices/0/message/reasoning_content"),
            Some(&json!("let me think"))
        );
        assert_eq!(
            out.pointer("/choices/0/message/tool_calls/0/function/name"),
            Some(&json!("f"))
        );
        assert_eq!(out.pointer("/usage/prompt_tokens"), Some(&json!(4)));
    }
}
