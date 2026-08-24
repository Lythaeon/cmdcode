//! Native Google Gemini API upstream adapter.
//!
//! Converts the internal OpenAI-format request into Gemini's
//! `:generateContent` / `:streamGenerateContent` wire format and translates
//! Gemini responses back into the OpenAI-format chunks downstream expects.
//!
//! Configure with `type: "gemini"` in providers.json:
//!
//! ```json
//! {"providers": {"google-direct": {
//!     "type": "gemini",
//!     "options": {"apiKey": "{env:GEMINI_API_KEY}"}
//! }}}
//! ```

use super::{Provider, RequestContext};
use crate::upstream::{LineOutcome, StreamState};
use cmdcode_core::auth::AuthManager;
use cmdcode_core::error::UpstreamError;
use cmdcode_core::wire_format::OpenAiMessage;
use serde_json::{json, Value};

pub(crate) const DEFAULT_BASE: &str = "https://generativelanguage.googleapis.com/v1beta";

/// Native Gemini provider.
#[derive(Clone)]
pub struct GeminiProvider {
    /// Base URL including `/v1beta`.
    pub base_url: String,
    /// API key.
    pub api_key: Option<String>,
}

// --- Request conversion (OpenAI internal -> Gemini) ------------------------

fn parts_from_content(content: &Option<Value>) -> Vec<Value> {
    match content {
        Some(Value::String(s)) => vec![json!({"text": s})],
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|p| {
                let text = p.get("text").and_then(|t| t.as_str())?;
                Some(json!({"text": text}))
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Convert internal messages to Gemini `contents` + `systemInstruction`.
fn convert_messages(messages: &[OpenAiMessage]) -> (String, Vec<Value>) {
    let mut system_parts: Vec<String> = Vec::new();
    let mut contents: Vec<Value> = Vec::new();

    for msg in messages {
        match msg.role.as_str() {
            "system" => system_parts.push(
                msg.content
                    .as_ref()
                    .and_then(|c| c.as_str())
                    .unwrap_or_default()
                    .to_string(),
            ),
            "assistant" => {
                let mut parts: Vec<Value> = parts_from_content(&msg.content);
                if let Some(calls) = &msg.tool_calls {
                    for call in calls {
                        let name = call
                            .function
                            .as_ref()
                            .and_then(|f| f.name.clone())
                            .unwrap_or_default();
                        let args_str = call
                            .function
                            .as_ref()
                            .and_then(|f| f.arguments.clone())
                            .unwrap_or_else(|| "{}".into());
                        let args: Value =
                            serde_json::from_str(&args_str).unwrap_or_else(|_| json!({}));
                        parts.push(json!({"functionCall": {"name": name, "args": args}}));
                    }
                }
                contents.push(json!({"role": "model", "parts": parts}));
            }
            "tool" => {
                // Tool results become functionResponse parts on a user turn.
                let name = msg
                    .tool_call_id
                    .clone()
                    .unwrap_or_default()
                    .trim_start_matches("call_")
                    .to_string();
                let result_text = msg
                    .content
                    .as_ref()
                    .and_then(|c| c.as_str())
                    .unwrap_or_default()
                    .to_string();
                contents.push(json!({
                    "role": "user",
                    "parts": [{"functionResponse": {
                        "name": name,
                        "response": {"result": result_text},
                    }}],
                }));
            }
            _ => {
                contents.push(json!({
                    "role": "user",
                    "parts": parts_from_content(&msg.content),
                }));
            }
        }
    }

    (system_parts.join("\n\n"), contents)
}

#[async_trait::async_trait]
impl Provider for GeminiProvider {
    fn name(&self) -> &'static str {
        "gemini"
    }

    fn endpoint(&self, model: &str, streaming: bool) -> String {
        let base = self.base_url.trim_end_matches('/');
        if streaming {
            format!("{base}/models/{model}:streamGenerateContent?alt=sse")
        } else {
            format!("{base}/models/{model}:generateContent")
        }
    }

    async fn headers(
        &self,
        _auth: &AuthManager,
        _cwd: &str,
    ) -> Result<Vec<(String, String)>, UpstreamError> {
        Ok(vec![
            ("Content-Type".into(), "application/json".into()),
            (
                "x-goog-api-key".into(),
                self.api_key.clone().unwrap_or_default(),
            ),
        ])
    }

    fn build_body(&self, ctx: &RequestContext<'_>) -> Value {
        let b = ctx.body;
        let (mut system, contents) = convert_messages(&b.messages);
        if let Some(taste) = &ctx.taste_section {
            system = if system.is_empty() {
                taste.clone()
            } else {
                format!("{taste}\n\n{system}")
            };
        }

        let mut body = json!({ "contents": contents });
        if !system.is_empty() {
            body["systemInstruction"] = json!({"parts": [{"text": system}]});
        }
        if let Some(effort) = ctx.effort {
            body["generationConfig"] = json!({
                "thinkingConfig": {"thinkingBudget": match effort.as_str() {
                    "low" => 0,
                    "high" | "xhigh" => 8192,
                    _ => 2048,
                }},
            });
        }
        // Merge sampling knobs into generationConfig (may already carry
        // thinkingConfig from the effort branch above).
        let mut gen_cfg: serde_json::Map<String, Value> = body
            .get("generationConfig")
            .and_then(|g| g.as_object())
            .cloned()
            .unwrap_or_default();
        if let Some(t) = b.temperature {
            gen_cfg.insert("temperature".into(), json!(t));
        }
        if let Some(p) = b.top_p {
            gen_cfg.insert("topP".into(), json!(p));
        }
        if let Some(mt) = b.max_tokens {
            gen_cfg.insert("maxOutputTokens".into(), json!(mt));
        }
        if let Some(stops) = &b.stop {
            gen_cfg.insert("stopSequences".into(), json!(stops));
        }
        body["generationConfig"] = Value::Object(gen_cfg);

        // Tools -> functionDeclarations.
        if let Some(tools) = &b.tools {
            let decls: Vec<Value> = tools
                .iter()
                .filter_map(|t| t.function.as_ref())
                .map(|f| {
                    json!({
                        "name": f.name,
                        "description": f.description.clone().unwrap_or_default(),
                        "parameters": f.parameters.clone().unwrap_or_else(|| json!({"type":"object"})),
                    })
                })
                .collect();
            if !decls.is_empty() {
                body["tools"] = json!([{ "functionDeclarations": decls }]);
            }
        }
        body
    }

    fn translate_line<'a>(
        &self,
        line: &str,
        state: &mut StreamState<'a>,
    ) -> LineOutcome {
        gemini_translate_line(line, state)
    }

    fn parse_non_streaming(
        &self,
        text: &str,
        model: &str,
    ) -> Result<Value, UpstreamError> {
        let parsed: Value = serde_json::from_str(text).map_err(|e| UpstreamError::HttpError {
            status: 502,
            body: format!("invalid gemini response: {e}"),
        })?;
        Ok(gemini_to_completion(&parsed, model))
    }

    fn is_auth_rejected(&self, status: u16) -> bool {
        status == 401 || status == 403
    }
}

/// Map a Gemini finishReason to the OpenAI finish reason.
fn finish_from_gemini(reason: &str) -> &'static str {
    match reason {
        "MAX_TOKENS" => "length",
        "SAFETY" | "RECITATION" => "content_filter",
        _ => "stop", // STOP and tool-call finishes both map to stop;
                     // tool_calls finish is synthesized when parts carry calls
    }
}

/// Convert a complete Gemini generateContent JSON into an OpenAI completion.
pub fn gemini_to_completion(resp: &Value, model: &str) -> Value {
    let cand = resp
        .pointer("/candidates/0")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let mut text_parts: Vec<String> = Vec::new();
    let mut tool_calls: Vec<Value> = Vec::new();

    if let Some(parts) = cand.pointer("/content/parts").and_then(|p| p.as_array()) {
        for part in parts {
            if let Some(t) = part.get("text").and_then(|t| t.as_str()) {
                text_parts.push(t.to_string());
            }
            if let Some(call) = part.get("functionCall") {
                tool_calls.push(json!({
                    "id": format!("call_{}", call.get("name").and_then(|n| n.as_str()).unwrap_or("unknown")),
                    "type": "function",
                    "function": {
                        "name": call.get("name").cloned().unwrap_or_else(|| json!("")),
                        "arguments": serde_json::to_string(
                            &call.get("args").cloned().unwrap_or_else(|| json!({}))
                        ).unwrap_or_default(),
                    },
                }));
            }
        }
    }

    let finish_reason = cand
        .get("finishReason")
        .and_then(|f| f.as_str())
        .map(finish_from_gemini)
        .unwrap_or("stop");

    // A tool-call turn reports finishReason STOP in Gemini; surface
    // tool_calls as the OpenAI finish reason when calls are present.
    let finish = if !tool_calls.is_empty() { "tool_calls" } else { finish_reason };

    let input = resp
        .pointer("/usageMetadata/promptTokenCount")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let output = resp
        .pointer("/usageMetadata/candidatesTokenCount")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    json!({
        "id": format!("chatcmpl-gemini-{}", resp.get("responseId").cloned().unwrap_or_else(|| json!(""))),
        "object": "chat.completion",
        "created": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        "model": model,
        "choices": [{
            "index": 0,
            "finish_reason": finish,
            "message": {
                "role": "assistant",
                "content": text_parts.join(""),
                "tool_calls": if tool_calls.is_empty() { Value::Null } else { json!(tool_calls) },
            },
        }],
        "usage": {
            "prompt_tokens": input,
            "completion_tokens": output,
            "total_tokens": input + output,
        },
    })
}

/// Translate one line of Gemini SSE (`alt=sse`) into OpenAI chunk frames.
pub fn gemini_translate_line<'a>(
    line: &str,
    state: &mut StreamState<'a>,
) -> LineOutcome {
    let trimmed = line.trim();
    if trimmed.is_empty() || !trimmed.starts_with("data:") {
        return LineOutcome::Skip;
    }
    let data = trimmed[5..].trim();
    let Ok(ev) = serde_json::from_str::<Value>(data) else {
        return LineOutcome::Skip;
    };

    let Some(cand) = ev.pointer("/candidates/0") else {
        return LineOutcome::Skip;
    };
    let parts = cand.pointer("/content/parts").and_then(|p| p.as_array());
    let finish_reason = cand.get("finishReason").and_then(|f| f.as_str());

    let mut deltas: Vec<Value> = Vec::new();
    if let Some(parts) = parts {
        for part in parts {
            if let Some(t) = part.get("text").and_then(|t| t.as_str()) {
                if !t.is_empty() {
                    deltas.push(json!({"content": t}));
                }
            }
            if let Some(call) = part.get("functionCall") {
                let name = call.get("name").and_then(|n| n.as_str()).unwrap_or("");
                let args = serde_json::to_string(&call.get("args").cloned().unwrap_or_else(|| json!({})))
                    .unwrap_or_default();
                deltas.push(json!({"tool_calls": [{
                    "index": 0,
                    "id": format!("call_{name}"),
                    "type": "function",
                    "function": {"name": name, "arguments": args},
                }]}));
            }
        }
    }

    let has_finish = finish_reason.is_some();
    if deltas.is_empty() && !has_finish {
        return LineOutcome::Skip;
    }

    let delta = if deltas.is_empty() { json!({}) } else {
        // Merge into a single choice delta.
        let mut content = String::new();
        let mut tool_calls: Vec<Value> = Vec::new();
        for d in deltas {
            if let Some(t) = d.get("content").and_then(|c| c.as_str()) {
                content.push_str(t);
            }
            if let Some(tc) = d.get("tool_calls").and_then(|c| c.as_array()) {
                tool_calls.extend(tc.iter().cloned());
            }
        }
        let mut m = json!({});
        if !content.is_empty() {
            m["content"] = json!(content);
        }
        if !tool_calls.is_empty() {
            m["tool_calls"] = json!(tool_calls);
        }
        m
    };

    let mut frame = json!({
        "id": state.completion_id,
        "object": "chat.completion.chunk",
        "model": state.model,
        "choices": [{
            "index": 0,
            "delta": delta,
            "finish_reason": finish_reason.map(finish_from_gemini),
        }],
    });

    if let Some(u) = ev.get("usageMetadata") {
        frame["usage"] = json!({
            "prompt_tokens": u.get("promptTokenCount").cloned().unwrap_or_else(|| json!(0)),
            "completion_tokens": u.get("candidatesTokenCount").cloned().unwrap_or_else(|| json!(0)),
            "total_tokens": u.get("totalTokenCount").cloned().unwrap_or_else(|| json!(0)),
        });
    }

    let payload = format!("data: {}\n\n", serde_json::to_string(&frame).unwrap_or_default());
    if has_finish {
        LineOutcome::EmitAndStop(payload)
    } else {
        LineOutcome::Emit(payload)
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
    fn test_build_body_conversions() {
        let body: ChatCompletionRequest = serde_json::from_value(json!({
            "model": "x",
            "messages": [
                {"role": "system", "content": "Be terse."},
                {"role": "user", "content": "Weather in Paris?"},
                {"role": "assistant", "content": null, "tool_calls": [
                    {"id": "call_get_weather", "type": "function",
                     "function": {"name": "get_weather", "arguments": "{\"city\":\"Paris\"}"}}
                ]},
                {"role": "tool", "tool_call_id": "call_get_weather", "content": "22C"}
            ],
            "tools": [{"type": "function", "function": {
                "name": "get_weather", "parameters": {"type": "object"}}}],
            "temperature": 0.3
        }))
        .unwrap();
        let provider = GeminiProvider {
            base_url: DEFAULT_BASE.into(),
            api_key: Some("k".into()),
        };
        let model = ModelId::new("gemini-2.5-pro");
        let out = provider.build_body(&ctx(&body, &model));
        assert_eq!(
            out["systemInstruction"]["parts"][0]["text"],
            "Be terse."
        );
        // [0]=user, [1]=model(functionCall), [2]=user(functionResponse)
        assert_eq!(out["contents"][1]["role"], "model");
        assert_eq!(
            out["contents"][1]["parts"][0]["functionCall"]["name"],
            "get_weather"
        );
        assert_eq!(
            out["contents"][2]["parts"][0]["functionResponse"]["name"],
            "get_weather"
        );
        assert_eq!(out["tools"][0]["functionDeclarations"][0]["name"], "get_weather");
        assert_eq!(body.temperature, Some(0.3));
    }

    #[test]
    fn test_translate_line_stream() {
        let mut state = StreamState {
            completion_id: "chatcmpl-t",
            created: 0,
            model: "gemini-2.5-pro",
            tool_index: 0,
            skipped: 0,
            finish_seen: false,
        };

        // Text chunk.
        let line = r#"data: {"candidates":[{"content":{"role":"model","parts":[{"text":"He"}]},"index":0}]}"#;
        match gemini_translate_line(line, &mut state) {
            LineOutcome::Emit(frame) => {
                let parsed: Value = serde_json::from_str(frame.trim_start_matches("data: ").trim()).unwrap();
                assert_eq!(parsed.pointer("/choices/0/delta/content"), Some(&json!("He")));
            }
            _ => panic!("expected Emit"),
        }

        // Function call chunk.
        let line = r#"data: {"candidates":[{"content":{"parts":[
            {"functionCall":{"name":"get_weather","args":{"city":"Paris"}}}]},"index":0}]}"#;
        match gemini_translate_line(line, &mut state) {
            LineOutcome::Emit(frame) => {
                let parsed: Value = serde_json::from_str(frame.trim_start_matches("data: ").trim()).unwrap();
                assert_eq!(
                    parsed.pointer("/choices/0/delta/tool_calls/0/function/name"),
                    Some(&json!("get_weather"))
                );
                assert_eq!(
                    parsed.pointer("/choices/0/delta/tool_calls/0/function/arguments"),
                    Some(&json!("{\"city\":\"Paris\"}"))
                );
            }
            _ => panic!("expected Emit"),
        }

        // Finish with usage -> EmitAndStop.
        let fin = r#"data: {"candidates":[{"content":{"role":"model","parts":[]},
            "finishReason":"STOP","index":0}],
            "usageMetadata":{"promptTokenCount":4,"candidatesTokenCount":6,"totalTokenCount":10}}"#;
        match gemini_translate_line(fin, &mut state) {
            LineOutcome::EmitAndStop(frame) => {
                // [DONE] is appended by the stream loop; frame is the final chunk.
                let payload = frame.trim_start_matches("data: ").trim();
                let parsed: Value = serde_json::from_str(payload).unwrap();
                assert_eq!(parsed.pointer("/choices/0/finish_reason"), Some(&json!("stop")));
                assert_eq!(parsed.pointer("/usage/prompt_tokens"), Some(&json!(4)));
            }
            _ => panic!("expected EmitAndStop"),
        }
    }

    #[test]
    fn test_parse_non_streaming() {
        let provider = GeminiProvider {
            base_url: String::new(),
            api_key: None,
        };
        let resp = json!({
            "candidates": [{
                "content": {"role": "model", "parts": [{"text": "22C"}]},
                "finishReason": "STOP"
            }],
            "usageMetadata": {"promptTokenCount": 5, "candidatesTokenCount": 2, "totalTokenCount": 7}
        });
        let out = provider
            .parse_non_streaming(&serde_json::to_string(&resp).unwrap(), "gemini")
            .unwrap();
        assert_eq!(out.pointer("/choices/0/message/content"), Some(&json!("22C")));
        assert_eq!(out.pointer("/usage/total_tokens"), Some(&json!(7)));
    }
}
