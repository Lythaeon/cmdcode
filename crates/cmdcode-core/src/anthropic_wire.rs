//! Anthropic Messages API (`/v1/messages`) wire types and conversions.
//!
//! Bridges the Anthropic protocol onto the proxy's internal OpenAI-format
//! representation so any configured upstream provider can serve Anthropic
//! clients.

use crate::wire_format::{
    ChatCompletionRequest, OpenAiFunction, OpenAiFunctionRef, OpenAiMessage, OpenAiTool,
    OpenAiToolCall,
};
use serde::Deserialize;
use serde_json::{json, Value};

/// Content block in an Anthropic message.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum AnthropicBlock {
    /// Plain text.
    #[serde(rename = "text")]
    Text {
        /// The text content.
        text: String,
    },
    /// Image (passed through as-is on conversion).
    #[serde(rename = "image")]
    Image {
        /// Remaining image fields (source etc.), passed through raw.
        #[serde(flatten)]
        extra: Value,
    },
    /// Tool result returned by the client.
    #[serde(rename = "tool_result")]
    ToolResult {
        /// ID of the tool_use block this responds to.
        tool_use_id: String,
        /// Result payload (string or content blocks).
        #[serde(default)]
        content: Value,
    },
    /// Catch-all for unknown blocks (thinking, etc.) — preserved raw.
    #[serde(untagged)]
    Other(Value),
}

/// One Anthropic message; `content` is a string or a block array.
#[derive(Debug, Clone, Deserialize)]
pub struct AnthropicMessage {
    pub role: String,
    pub content: Value,
}

/// Flat Anthropic tool definition.
#[derive(Debug, Clone, Deserialize)]
pub struct AnthropicTool {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub input_schema: Value,
}

/// Incoming `/v1/messages` request body.
#[derive(Debug, Clone, Deserialize)]
pub struct AnthropicRequest {
    pub model: String,
    /// Required by Anthropic; forwarded as max_tokens.
    pub max_tokens: u64,
    /// Top-level system prompt (string or text-block array).
    #[serde(default)]
    pub system: Option<Value>,
    pub messages: Vec<AnthropicMessage>,
    #[serde(default)]
    pub tools: Option<Vec<AnthropicTool>>,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub top_p: Option<f64>,
    #[serde(default)]
    pub stop_sequences: Option<Vec<String>>,
    #[serde(default)]
    pub stream: Option<bool>,
    #[serde(default)]
    pub metadata: Option<Value>,
}

/// Flatten Anthropic content (string or blocks) into OpenAI content JSON.
fn flatten_content(content: &Value) -> Value {
    if content.is_string() {
        return content.clone();
    }
    let Some(blocks) = content.as_array() else {
        return content.clone();
    };
    let mut parts: Vec<Value> = Vec::new();
    for block in blocks {
        match block.get("type").and_then(|t| t.as_str()) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                    parts.push(json!({"type": "text", "text": text}));
                }
            }
            // Images pass through structurally.
            Some("image") => parts.push(block.clone()),
            Some("tool_result") => {
                // Converted at message level below; keep marker here too.
                parts.push(block.clone());
            }
            _ => {}
        }
    }
    Value::Array(parts)
}

impl AnthropicRequest {
    /// Convert to the internal OpenAI-format request.
    pub fn to_chat_completion(&self) -> ChatCompletionRequest {
        let mut messages: Vec<OpenAiMessage> = Vec::new();

        if let Some(system) = &self.system {
            let text = match system {
                Value::String(s) => s.clone(),
                Value::Array(blocks) => blocks
                    .iter()
                    .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n"),
                _ => String::new(),
            };
            if !text.is_empty() {
                messages.push(OpenAiMessage {
                    role: "system".into(),
                    content: Some(Value::String(text)),
                    tool_calls: None,
                    tool_call_id: None,
                });
            }
        }

        // Track pending tool results: Anthropic sends them inside the NEXT
        // user message's blocks; OpenAI wants one `tool` role message each.
        for msg in &self.messages {
            let role = msg.role.as_str();
            if role == "user" {
                if let Some(blocks) = msg.content.as_array() {
                    let has_tool_results = blocks.iter().any(|b| {
                        b.get("type").and_then(|t| t.as_str()) == Some("tool_result")
                    });
                    if has_tool_results {
                        for block in blocks {
                            if block.get("type").and_then(|t| t.as_str()) != Some("tool_result")
                            {
                                continue;
                            }
                            let id = block
                                .get("tool_use_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default();
                            let content = match block.get("content") {
                                Some(Value::String(s)) => s.clone(),
                                Some(Value::Array(parts)) => parts
                                    .iter()
                                    .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                                    .collect::<Vec<_>>()
                                    .join("\n"),
                                _ => String::new(),
                            };
                            messages.push(OpenAiMessage {
                                role: "tool".into(),
                                content: Some(Value::String(content)),
                                tool_calls: None,
                                tool_call_id: Some(id.to_string()),
                            });
                        }
                        continue;
                    }
                }
                messages.push(OpenAiMessage {
                    role: "user".into(),
                    content: Some(flatten_content(&msg.content)),
                    tool_calls: None,
                    tool_call_id: None,
                });
            } else if role == "assistant" {
                // Assistant messages may carry tool_use blocks -> tool_calls.
                let mut tool_calls: Vec<OpenAiToolCall> = Vec::new();
                let mut text_parts: Vec<String> = Vec::new();
                if let Some(blocks) = msg.content.as_array() {
                    for block in blocks {
                        match block.get("type").and_then(|t| t.as_str()) {
                            Some("text") => {
                                if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                                    text_parts.push(t.to_string());
                                }
                            }
                            Some("tool_use") => {
                                let args = serde_json::to_string(
                                    &block.get("input").cloned().unwrap_or(json!({})),
                                )
                                .unwrap_or_default();
                                tool_calls.push(OpenAiToolCall {
                                    id: block
                                        .get("id")
                                        .and_then(|v| v.as_str())
                                        .map(String::from),
                                    function: Some(OpenAiFunctionRef {
                                        name: block
                                            .get("name")
                                            .and_then(|n| n.as_str())
                                            .map(String::from),
                                        arguments: Some(args),
                                    }),
                                });
                            }
                            _ => {}
                        }
                    }
                } else if let Some(s) = msg.content.as_str() {
                    text_parts.push(s.to_string());
                }
                let content = if text_parts.is_empty() && !tool_calls.is_empty() {
                    None
                } else {
                    Some(Value::String(text_parts.join("")))
                };
                messages.push(OpenAiMessage {
                    role: "assistant".into(),
                    content,
                    tool_calls: if tool_calls.is_empty() {
                        None
                    } else {
                        Some(tool_calls)
                    },
                    tool_call_id: None,
                });
            } else {
                messages.push(OpenAiMessage {
                    role: role.to_string(),
                    content: Some(flatten_content(&msg.content)),
                    tool_calls: None,
                    tool_call_id: None,
                });
            }
        }

        ChatCompletionRequest {
            model: Some(self.model.clone()),
            messages,
            tools: self.tools.as_ref().map(|tools| {
                tools
                    .iter()
                    .map(|t| OpenAiTool {
                        tool_type: "function".into(),
                        function: Some(OpenAiFunction {
                            name: t.name.clone(),
                            description: t.description.clone(),
                            parameters: if t.input_schema.is_null() {
                                None
                            } else {
                                Some(t.input_schema.clone())
                            },
                        }),
                        name: None,
                        description: None,
                        input_schema: None,
                        parameters: None,
                    })
                    .collect()
            }),
            max_tokens: Some(self.max_tokens.min(u32::MAX as u64) as u32),
            temperature: self.temperature,
            top_p: self.top_p,
            frequency_penalty: None,
            presence_penalty: None,
            stop: self.stop_sequences.clone(),
            user: None,
            n: None,
            seed: None,
            response_format: None,
            logprobs: None,
            top_logprobs: None,
            reasoning_effort: None,
            stream_options: None,
            stream: self.stream,
        }
    }
}

/// Map an OpenAI finish reason to an Anthropic stop reason.
pub fn stop_reason_from_openai(reason: &str) -> &'static str {
    match reason {
        "length" => "max_tokens",
        "tool_calls" | "function_call" => "tool_use",
        "content_filter" => "refusal",
        _ => "end_turn",
    }
}

/// Convert an internal OpenAI completion JSON into an Anthropic response.
pub fn completion_to_anthropic(openai: &Value, model: &str) -> Value {
    let choice = openai
        .pointer("/choices/0")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let message = choice.get("message").cloned().unwrap_or_else(|| json!({}));

    let mut content: Vec<Value> = Vec::new();

    // Reasoning first (Anthropic puts thinking before text).
    if let Some(reasoning) = message
        .get("reasoning_content")
        .and_then(|r| r.as_str())
        .filter(|s| !s.is_empty())
    {
        content.push(json!({"type": "thinking", "thinking": reasoning}));
    }

    if let Some(calls) = message.get("tool_calls").and_then(|c| c.as_array()) {
        for call in calls {
            let args_str = call
                .pointer("/function/arguments")
                .and_then(|a| a.as_str())
                .unwrap_or("{}");
            let input: Value = serde_json::from_str(args_str).unwrap_or(json!({}));
            content.push(json!({
                "type": "tool_use",
                "id": call.get("id").cloned().unwrap_or(json!("call_unknown")),
                "name": call.pointer("/function/name").cloned().unwrap_or(json!("")),
                "input": input,
            }));
        }
    }

    let text = message
        .get("content")
        .and_then(|c| c.as_str())
        .unwrap_or_default();
    if !text.is_empty() || content.is_empty() {
        content.push(json!({"type": "text", "text": text}));
    }

    let finish_reason = choice
        .get("finish_reason")
        .and_then(|f| f.as_str())
        .unwrap_or("stop");

    let input_tokens = openai
        .pointer("/usage/prompt_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let output_tokens = openai
        .pointer("/usage/completion_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    json!({
        "id": openai.get("id").cloned().unwrap_or_else(|| json!("msg_cmdcode")),
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": content,
        "stop_reason": stop_reason_from_openai(finish_reason),
        "stop_sequence": Value::Null,
        "usage": {
            "input_tokens": input_tokens,
            "output_tokens": output_tokens,
        },
    })
}

/// Renders provider-emitted OpenAI SSE chunks into the Anthropic event
/// sequence (`message_start`, `content_block_*`, `message_delta`,
/// `message_stop`).
///
/// Feed it every payload line from the upstream rx channel; it returns zero
/// or more complete SSE frames ready to write to the client.
#[derive(Debug, Default)]
pub struct AnthropicStreamRenderer {
    started: bool,
    finished: bool,
    /// Index of the currently-open content block (None = none open).
    open_block: Option<(usize, BlockKind)>,
    next_index: usize,
    output_tokens_estimate: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockKind {
    Text,
    Thinking,
    ToolUse,
}

impl AnthropicStreamRenderer {
    /// Create a fresh renderer for one streamed response.
    pub fn new() -> Self {
        Self::default()
    }

    fn frame(event: &str, data: Value) -> String {
        format!(
            "event: {}\ndata: {}\n\n",
            event,
            serde_json::to_string(&data).unwrap_or_default()
        )
    }

    fn close_block(&mut self, out: &mut Vec<String>) {
        if let Some((index, kind)) = self.open_block.take() {
            out.push(Self::frame(
                "content_block_stop",
                json!({"type": "content_block_stop", "index": index}),
            ));
            let _ = kind;
        }
    }

    /// Consume one OpenAI SSE payload (`data: {...}` or `data: [DONE]`),
    /// returning rendered Anthropic SSE frames.
    pub fn feed(&mut self, payload: &str) -> Vec<String> {
        let mut out = Vec::new();
        let payload = payload.trim();
        let data = payload.strip_prefix("data:").unwrap_or(payload).trim();
        if data.is_empty() {
            return out;
        }
        if data == "[DONE]" {
            // Ensure termination even if no finish_reason chunk arrived.
            if !self.finished {
                self.close_block(&mut out);
                out.push(Self::frame(
                    "message_delta",
                    json!({
                        "type": "message_delta",
                        "delta": {"stop_reason": "end_turn", "stop_sequence": null},
                        "usage": {"output_tokens": self.output_tokens_estimate},
                    }),
                ));
                out.push(Self::frame("message_stop", json!({"type": "message_stop"})));
                self.finished = true;
            }
            return out;
        }

        let Ok(chunk) = serde_json::from_str::<Value>(data) else {
            return out;
        };

        if !self.started {
            self.started = true;
            out.push(Self::frame(
                "message_start",
                json!({
                    "type": "message_start",
                    "message": {
                        "id": chunk.get("id").cloned().unwrap_or_else(|| json!(format!(
                            "msg_{}",
                            uuid::Uuid::new_v4()
                        ))),
                        "type": "message",
                        "role": "assistant",
                        "model": chunk.get("model").cloned().unwrap_or(json!("")),
                        "content": [],
                        "stop_reason": Value::Null,
                        "usage": {"input_tokens": 0, "output_tokens": 0},
                    },
                }),
            ));
        }

        // Usage may appear on any chunk; remember the latest.
        if let Some(u) = chunk.get("usage") {
            if let Some(n) = u.get("completion_tokens").and_then(|v| v.as_u64()) {
                self.output_tokens_estimate = n;
            }
        }

        let choices = chunk.get("choices");
        if let Some(choice) = choices.and_then(|c| c.as_array()).and_then(|a| a.first()) {
            // Reasoning deltas.
            if let Some(delta) = choice.get("delta") {
                if let Some(reasoning) = delta
                    .get("reasoning_content")
                    .and_then(|r| r.as_str())
                    .filter(|s| !s.is_empty())
                {
                    if self.open_block.map(|(_, k)| k) != Some(BlockKind::Thinking) {
                        self.close_block(&mut out);
                        let index = self.next_index;
                        self.next_index += 1;
                        self.open_block = Some((index, BlockKind::Thinking));
                        out.push(Self::frame(
                            "content_block_start",
                            json!({
                                "type": "content_block_start",
                                "index": index,
                                "content_block": {"type": "thinking", "thinking": ""},
                            }),
                        ));
                    }
                    let (index, _) = self.open_block.expect("just opened");
                    out.push(Self::frame(
                        "content_block_delta",
                        json!({
                            "type": "content_block_delta",
                            "index": index,
                            "delta": {"type": "thinking_delta", "thinking": reasoning},
                        }),
                    ));
                }

                // Text deltas.
                if let Some(text) = delta
                    .get("content")
                    .and_then(|c| c.as_str())
                    .filter(|s| !s.is_empty())
                {
                    if self.open_block.map(|(_, k)| k) != Some(BlockKind::Text) {
                        self.close_block(&mut out);
                        let index = self.next_index;
                        self.next_index += 1;
                        self.open_block = Some((index, BlockKind::Text));
                        out.push(Self::frame(
                            "content_block_start",
                            json!({
                                "type": "content_block_start",
                                "index": index,
                                "content_block": {"type": "text", "text": ""},
                            }),
                        ));
                    }
                    let (index, _) = self.open_block.expect("just opened");
                    out.push(Self::frame(
                        "content_block_delta",
                        json!({
                            "type": "content_block_delta",
                            "index": index,
                            "delta": {"type": "text_delta", "text": text},
                        }),
                    ));
                }

                // Tool-call deltas: fragments keyed by index.
                if let Some(calls) = delta.get("tool_calls").and_then(|c| c.as_array()) {
                    for call in calls {
                        let name = call.pointer("/function/name").and_then(|n| n.as_str());
                        let args = call.pointer("/function/arguments").and_then(|a| a.as_str());
                        let id = call.get("id").and_then(|i| i.as_str());
                        if name.is_some() || id.is_some() {
                            // New tool block starts.
                            self.close_block(&mut out);
                            let index = self.next_index;
                            self.next_index += 1;
                            self.open_block = Some((index, BlockKind::ToolUse));
                            out.push(Self::frame(
                                "content_block_start",
                                json!({
                                    "type": "content_block_start",
                                    "index": index,
                                    "content_block": {
                                        "type": "tool_use",
                                        "id": id.unwrap_or("call_unknown"),
                                        "name": name.unwrap_or(""),
                                        "input": {},
                                    },
                                }),
                            ));
                        }
                        if let Some(args) = args.filter(|a| !a.is_empty()) {
                            let (index, _) =
                                self.open_block.unwrap_or((self.next_index, BlockKind::ToolUse));
                            out.push(Self::frame(
                                "content_block_delta",
                                json!({
                                    "type": "content_block_delta",
                                    "index": index,
                                    "delta": {"type": "input_json_delta", "partial_json": args},
                                }),
                            ));
                        }
                    }
                }
            }

            // Finish.
            if let Some(reason) = choice
                .get("finish_reason")
                .and_then(|r| r.as_str())
                .map(str::to_string)
            {
                self.close_block(&mut out);
                out.push(Self::frame(
                    "message_delta",
                    json!({
                        "type": "message_delta",
                        "delta": {
                            "stop_reason": stop_reason_from_openai(&reason),
                            "stop_sequence": null,
                        },
                        "usage": {"output_tokens": self.output_tokens_estimate},
                    }),
                ));
                out.push(Self::frame("message_stop", json!({"type": "message_stop"})));
                self.finished = true;
            }
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_request() -> AnthropicRequest {
        serde_json::from_value(json!({
            "model": "claude-sonnet-5",
            "max_tokens": 1024,
            "system": "You are helpful.",
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "Hello"}]},
                {"role": "assistant", "content": [
                    {"type": "tool_use", "id": "call_1", "name": "get_weather",
                     "input": {"city": "Paris"}}
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "call_1", "content": "22C"}
                ]}
            ],
            "tools": [{
                "name": "get_weather",
                "description": "Get weather",
                "input_schema": {"type": "object", "properties": {"city": {"type": "string"}}}
            }],
            "stream": true
        }))
        .unwrap()
    }

    #[test]
    fn test_to_chat_completion_system_and_tools() {
        let cc = sample_request().to_chat_completion();
        assert_eq!(cc.model.as_deref(), Some("claude-sonnet-5"));
        assert_eq!(cc.max_tokens, Some(1024));
        assert_eq!(cc.stream, Some(true));

        // System message first.
        assert_eq!(cc.messages[0].role, "system");
        assert!(matches!(
            &cc.messages[0].content,
            Some(Value::String(s)) if s == "You are helpful."
        ));

        // Tools flattened to OpenAI function form.
        let tool = &cc.tools.as_ref().unwrap()[0];
        assert_eq!(tool.tool_type, "function");
        assert_eq!(
            tool.function.as_ref().unwrap().name,
            "get_weather"
        );
    }

    #[test]
    fn test_tool_result_becomes_tool_role() {
        let cc = sample_request().to_chat_completion();
        // user text -> assistant(tool_use) -> tool(result)
        assert_eq!(cc.messages[1].role, "user");
        assert_eq!(cc.messages[2].role, "assistant");
        assert_eq!(cc.messages[2].tool_calls.as_ref().unwrap()[0]
            .function.as_ref().unwrap().name.as_deref(),
            Some("get_weather")
        );
        assert_eq!(cc.messages[3].role, "tool");
        assert_eq!(
            cc.messages[3].tool_call_id.as_deref(),
            Some("call_1")
        );
    }

    #[test]
    fn test_completion_to_anthropic_text() {
        let openai = json!({
            "id": "chatcmpl-x",
            "choices": [{
                "finish_reason": "stop",
                "message": {"role": "assistant", "content": "Hi there"}
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5},
        });
        let out = completion_to_anthropic(&openai, "claude-sonnet-5");
        assert_eq!(out["type"], "message");
        assert_eq!(out["stop_reason"], "end_turn");
        assert_eq!(out["content"][0]["type"], "text");
        assert_eq!(out["content"][0]["text"], "Hi there");
        assert_eq!(out["usage"]["input_tokens"], 10);
    }

    #[test]
    fn test_completion_to_anthropic_tool_use() {
        let openai = json!({
            "choices": [{
                "finish_reason": "tool_calls",
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_9",
                        "type": "function",
                        "function": {"name": "f", "arguments": "{\"x\":1}"}
                    }]
                }
            }],
        });
        let out = completion_to_anthropic(&openai, "m");
        assert_eq!(out["stop_reason"], "tool_use");
        assert_eq!(out["content"][0]["type"], "tool_use");
        assert_eq!(out["content"][0]["input"]["x"], 1);
    }

    fn openai_text_chunk(text: &str) -> String {
        format!(
            "data: {}",
            json!({"id": "chatcmpl-m", "model": "m", "choices": [
                {"delta": {"content": text}, "finish_reason": null}
            ]})
        )
    }

    #[test]
    fn test_stream_renderer_event_sequence() {
        let mut r = AnthropicStreamRenderer::new();

        let c1 = openai_text_chunk("Hel");
        let frames = r.feed(&c1);
        let joined = frames.join("");
        assert!(joined.contains("event: message_start"));
        assert!(joined.contains("event: content_block_start"));
        assert!(joined.contains("\"type\":\"text_delta\""));

        let c2 = openai_text_chunk("lo");
        let frames = r.feed(&c2);
        assert!(frames.join("").contains("content_block_delta"));

        let done = r.feed("data: [DONE]");
        let tail = done.join("");
        assert!(tail.contains("message_delta"));
        assert!(tail.contains("message_stop"));
    }

    #[test]
    fn test_stream_renderer_tool_call_blocks() {
        let mut r = AnthropicStreamRenderer::new();

        let start = json!({
            "choices": [{"delta": {"tool_calls": [
                {"index": 0, "id": "call_x", "function": {"name": "write_taste_file"}}
            ]}}]
        });
        let frames = r.feed(&format!("data: {start}"));
        let joined = frames.join("");
        assert!(joined.contains("\"type\":\"tool_use\""));
        assert!(joined.contains("write_taste_file"));

        let delta = json!({
            "choices": [{"delta": {"tool_calls": [
                {"index": 0, "function": {"arguments": "{\"path\":"}}
            ]}}]
        });
        let frames = r.feed(&format!("data: {delta}"));
        assert!(frames.join("").contains("input_json_delta"));
    }

    #[test]
    fn test_stop_reason_mapping() {
        assert_eq!(stop_reason_from_openai("stop"), "end_turn");
        assert_eq!(stop_reason_from_openai("length"), "max_tokens");
        assert_eq!(stop_reason_from_openai("tool_calls"), "tool_use");
        assert_eq!(stop_reason_from_openai("content_filter"), "refusal");
    }
}
