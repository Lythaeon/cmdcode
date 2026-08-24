//! OpenAI Responses API (`/v1/responses`) wire protocol.
//!
//! Implements the stateless subset: `input` as a string or a message/item
//! array, function tools, streaming events. Server-side state
//! (`previous_response_id`) is not persisted — clients relying on it should
//! send full input each turn.

use crate::wire_format::{
    ChatCompletionRequest, OpenAiFunction, OpenAiFunctionRef, OpenAiMessage, OpenAiTool,
    OpenAiToolCall,
};
use serde::Deserialize;
use serde_json::{json, Value};

/// One input item of the Responses API.
#[derive(Debug, Clone, Deserialize)]
pub struct ResponseInputItem {
    /// Item type (`message`, `function_call`, `function_call_output`).
    #[serde(rename = "type", default)]
    pub item_type: Option<String>,
    /// Message role.
    #[serde(default)]
    pub role: Option<String>,
    /// Content: string or typed parts (`input_text`, `output_text`).
    #[serde(default)]
    pub content: Value,
    /// Function call identifier.
    #[serde(default, alias = "call_id")]
    pub id: Option<String>,
    /// Called function name (function_call items).
    #[serde(default)]
    pub name: Option<String>,
    /// JSON-encoded arguments (function_call items).
    #[serde(default)]
    pub arguments: Option<String>,
    /// Tool result output (function_call_output items).
    #[serde(default)]
    pub output: Option<Value>,
}

/// Flat Responses-API tool definition.
#[derive(Debug, Clone, Deserialize)]
pub struct ResponseTool {
    /// Only `function` is supported; others are ignored.
    #[serde(rename = "type", default)]
    pub tool_type: Option<String>,
    /// Function name.
    #[serde(default)]
    pub name: Option<String>,
    /// What it does.
    #[serde(default)]
    pub description: Option<String>,
    /// JSON Schema for parameters.
    #[serde(default)]
    pub parameters: Value,
}


/// Incoming `/v1/responses` request body.
#[derive(Debug, Clone, Deserialize)]
pub struct ResponsesRequest {
    /// Requested model id.
    pub model: String,
    /// Input: plain string or an array of items.
    pub input: Value,
    /// System-style instructions (prepended as system message).
    #[serde(default)]
    pub instructions: Option<Value>,
    /// Available tools (flat definitions).
    #[serde(default)]
    pub tools: Option<Vec<ResponseTool>>,
    /// Whether to stream events.
    #[serde(default)]
    pub stream: Option<bool>,
    /// Output token cap.
    #[serde(rename = "max_output_tokens", default)]
    pub max_output_tokens: Option<u32>,
    /// Sampling temperature.
    #[serde(default)]
    pub temperature: Option<f64>,
    /// Nucleus sampling threshold.
    #[serde(default)]
    pub top_p: Option<f64>,
}

/// Extract text from message content (string or parts array).
fn content_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

impl ResponsesRequest {
    /// Convert to the internal OpenAI-format request.
    pub fn to_chat_completion(&self) -> ChatCompletionRequest {
        let mut messages: Vec<OpenAiMessage> = Vec::new();

        if let Some(instructions) = &self.instructions {
            let text = content_text(instructions);
            if !text.is_empty() {
                messages.push(OpenAiMessage {
                    role: "system".into(),
                    content: Some(json!(text)),
                    tool_calls: None,
                    tool_call_id: None,
                });
            }
        }

        match &self.input {
            // Plain string shorthand.
            Value::String(text) => messages.push(OpenAiMessage {
                role: "user".into(),
                content: Some(json!(text.clone())),
                tool_calls: None,
                tool_call_id: None,
            }),
            Value::Array(items) => {
                for item in items {
                    let item_type = item
                        .get("type")
                        .and_then(|t| t.as_str())
                        .unwrap_or("message");
                    match item_type {
                        "message" => {
                            let role = item
                                .get("role")
                                .and_then(|r| r.as_str())
                                .unwrap_or("user")
                                .to_string();
                            let content = item.get("content").cloned().unwrap_or_else(|| json!(""));
                            messages.push(OpenAiMessage {
                                role,
                                content: Some(json!(content_text(&content))),
                                tool_calls: None,
                                tool_call_id: None,
                            });
                        }
                        "function_call" => {
                            let id = item
                                .get("call_id")
                                .or_else(|| item.get("id"))
                                .and_then(|v| v.as_str())
                                .map(String::from);
                            messages.push(OpenAiMessage {
                                role: "assistant".into(),
                                content: None,
                                tool_calls: Some(vec![OpenAiToolCall {
                                    id,
                                    function: Some(OpenAiFunctionRef {
                                        name: item
                                            .get("name")
                                            .and_then(|n| n.as_str())
                                            .map(String::from),
                                        arguments: item
                                            .get("arguments")
                                            .and_then(|a| a.as_str())
                                            .map(String::from),
                                    }),
                                }]),
                                tool_call_id: None,
                            });
                        }
                        "function_call_output" => {
                            let output = match item.get("output") {
                                Some(Value::String(s)) => s.clone(),
                                Some(other) => other.to_string(),
                                None => String::new(),
                            };
                            messages.push(OpenAiMessage {
                                role: "tool".into(),
                                content: Some(json!(output)),
                                tool_calls: None,
                                tool_call_id: item
                                    .get("call_id")
                                    .and_then(|v| v.as_str())
                                    .map(String::from),
                            });
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }

        let tools = self.tools.as_ref().map(|tools| {
            tools
                .iter()
                .filter(|t| t.tool_type.as_deref() != Some("web_search"))
                .filter_map(|t| {
                    let name = t.name.clone()?;
                    Some(OpenAiTool {
                        tool_type: "function".into(),
                        function: Some(OpenAiFunction {
                            name,
                            description: t.description.clone(),
                            parameters: if t.parameters.is_null() {
                                None
                            } else {
                                Some(t.parameters.clone())
                            },
                        }),
                        name: None,
                        description: None,
                        input_schema: None,
                        parameters: None,
                    })
                })
                .collect::<Vec<_>>()
        });

        ChatCompletionRequest {
            model: Some(self.model.clone()),
            messages,
            tools,
            max_tokens: self.max_output_tokens,
            temperature: self.temperature,
            stream: self.stream,
            reasoning_effort: None,
            stream_options: None,
            frequency_penalty: None,
            presence_penalty: None,
            top_p: self.top_p,
            stop: None,
            user: None,
            n: None,
            seed: None,
            response_format: None,
            logprobs: None,
            top_logprobs: None,
        }
    }
}

/// Convert an internal OpenAI completion into a Responses-API response.
pub fn completion_to_responses(openai: &Value, model: &str) -> Value {
    let choice = openai
        .pointer("/choices/0")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let message = choice.get("message").cloned().unwrap_or_else(|| json!({}));

    let mut output: Vec<Value> = Vec::new();

    if let Some(reasoning) = message
        .get("reasoning_content")
        .and_then(|r| r.as_str())
        .filter(|s| !s.is_empty())
    {
        output.push(json!({
            "type": "reasoning",
            "summary": [],
            "content": [{"type": "reasoning_text", "text": reasoning}],
        }));
    }

    if let Some(calls) = message.get("tool_calls").and_then(|c| c.as_array()) {
        for call in calls {
            output.push(json!({
                "type": "function_call",
                "call_id": call.get("id").cloned().unwrap_or_else(|| json!("call_0")),
                "name": call.pointer("/function/name").cloned().unwrap_or_else(|| json!("")),
                "arguments": call.pointer("/function/arguments").cloned().unwrap_or_else(|| json!("{}")),
            }));
        }
    }

    let text = message
        .get("content")
        .and_then(|c| c.as_str())
        .unwrap_or_default();
    if !text.is_empty() || output.is_empty() {
        output.push(json!({
            "type": "message",
            "role": "assistant",
            "status": "completed",
            "content": [{"type": "output_text", "text": text, "annotations": []}],
        }));
    }

    let status = if choice.get("finish_reason").and_then(|f| f.as_str()) == Some("tool_calls") {
        json!({"type":"function_call","name":null})
    } else {
        Value::Null
    };
    let _ = status;

    json!({
        "id": openai.get("id").cloned().unwrap_or_else(|| json!("resp_cmdcode")),
        "object": "response",
        "created_at": openai.get("created").cloned().unwrap_or_else(|| json!(0)),
        "status": "completed",
        "model": model,
        "output": output,
        "usage": {
            "input_tokens": openai.pointer("/usage/prompt_tokens").cloned().unwrap_or_else(|| json!(0)),
            "output_tokens": openai.pointer("/usage/completion_tokens").cloned().unwrap_or_else(|| json!(0)),
            "total_tokens": openai.pointer("/usage/total_tokens").cloned().unwrap_or_else(|| json!(0)),
        },
    })
}

/// Renders provider-emitted OpenAI SSE chunks into Responses-API streaming
/// events.
///
/// Emits `response.created` on first chunk, `response.output_text.delta` for
/// each text piece, `response.function_call_arguments.delta` for tool args,
/// and `response.completed` at finish.
#[derive(Debug, Default)]
pub struct ResponsesStreamRenderer {
    created: bool,
    finished: bool,
}

impl ResponsesStreamRenderer {
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

    /// Consume one OpenAI SSE payload, returning rendered event frames.
    pub fn feed(&mut self, payload: &str) -> Vec<String> {
        let data = payload.trim().strip_prefix("data:").unwrap_or(payload).trim();
        if data.is_empty() || self.finished {
            return Vec::new();
        }
        if data == "[DONE]" {
            return Vec::new(); // completed emitted with finish_reason
        }
        let Ok(chunk) = serde_json::from_str::<Value>(data) else {
            return Vec::new();
        };

        let mut out = Vec::new();
        if !self.created {
            self.created = true;
            out.push(Self::frame(
                "response.created",
                json!({
                    "type": "response.created",
                    "response": {
                        "id": chunk.get("id").cloned().unwrap_or_else(|| json!("resp_cmdcode")),
                        "object": "response",
                        "status": "in_progress",
                        "model": chunk.get("model").cloned().unwrap_or_else(|| json!("")),
                        "output": [],
                    },
                }),
            ));
        }

        let Some(choice) = chunk.pointer("/choices/0") else {
            return out;
        };
        let delta = choice.get("delta").cloned().unwrap_or_else(|| json!({}));

        if let Some(reasoning) = delta
            .get("reasoning_content")
            .and_then(|r| r.as_str())
            .filter(|s| !s.is_empty())
        {
            out.push(Self::frame(
                "response.reasoning_summary_text.delta",
                json!({
                    "type": "response.reasoning_summary_text.delta",
                    "delta": reasoning,
                }),
            ));
        }

        if let Some(text) = delta
            .get("content")
            .and_then(|c| c.as_str())
            .filter(|s| !s.is_empty())
        {
            out.push(Self::frame(
                "response.output_text.delta",
                json!({
                    "type": "response.output_text.delta",
                    "delta": text,
                }),
            ));
        }

        if let Some(calls) = delta.get("tool_calls").and_then(|c| c.as_array()) {
            for call in calls {
                if let Some(args) = call
                    .pointer("/function/arguments")
                    .and_then(|a| a.as_str())
                    .filter(|a| !a.is_empty())
                {
                    out.push(Self::frame(
                        "response.function_call_arguments.delta",
                        json!({
                            "type": "response.function_call_arguments.delta",
                            "delta": args,
                        }),
                    ));
                }
            }
        }

        if let Some(finish) = choice
            .get("finish_reason")
            .and_then(|f| f.as_str())
            .map(str::to_string)
        {
            let stop_reason = if finish == "tool_calls" { "function_call" } else { &finish };
            let _ = stop_reason;
            out.push(Self::frame(
                "response.completed",
                json!({
                    "type": "response.completed",
                    "response": {
                        "id": chunk.get("id").cloned().unwrap_or_else(|| json!("resp_cmdcode")),
                        "object": "response",
                        "status": "completed",
                        "model": chunk.get("model").cloned().unwrap_or_else(|| json!("")),
                        "usage": {
                            "input_tokens": chunk.pointer("/usage/prompt_tokens").cloned().unwrap_or_else(|| json!(0)),
                            "output_tokens": chunk.pointer("/usage/completion_tokens").cloned().unwrap_or_else(|| json!(0)),
                            "total_tokens": chunk.pointer("/usage/total_tokens").cloned().unwrap_or_else(|| json!(0)),
                        },
                    },
                }),
            ));
            self.finished = true;
        }

        out
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_to_chat_completion_from_string_input() {
        let req: ResponsesRequest =
            serde_json::from_value(json!({"model": "gpt-5", "input": "Hello"})).unwrap();
        let cc = req.to_chat_completion();
        assert_eq!(cc.messages.len(), 1);
        assert_eq!(cc.messages[0].role, "user");
    }

    #[test]
    fn test_to_chat_completion_full_flow() {
        let req: ResponsesRequest = serde_json::from_value(json!({
            "model": "gpt-5",
            "instructions": "Be terse.",
            "input": [
                {"type": "message", "role": "user", "content": "Weather?"},
                {"type": "function_call", "call_id": "c1", "name": "get_weather",
                 "arguments": "{\"city\":\"Paris\"}"},
                {"type": "function_call_output", "call_id": "c1", "output": "22C"}
            ],
            "tools": [{"type": "function", "name": "get_weather",
                       "parameters": {"type": "object"}}],
            "stream": false
        }))
        .unwrap();
        let cc = req.to_chat_completion();
        assert_eq!(cc.messages[0].role, "system");
        assert_eq!(cc.messages[1].role, "user");
        assert_eq!(cc.messages[2].role, "assistant");
        assert_eq!(
            cc.messages[2].tool_calls.as_ref().unwrap()[0]
                .function
                .as_ref()
                .unwrap()
                .name
                .as_deref(),
            Some("get_weather")
        );
        assert_eq!(cc.messages[3].role, "tool");
        assert_eq!(cc.tools.as_ref().unwrap()[0]
            .function
            .as_ref()
            .unwrap()
            .name, "get_weather");
    }

    #[test]
    fn test_completion_to_responses() {
        let openai = json!({
            "choices": [{
                "finish_reason": "stop",
                "message": {"role": "assistant", "content": "Hi"}
            }],
            "usage": {"prompt_tokens": 3, "completion_tokens": 2, "total_tokens": 5},
        });
        let out = completion_to_responses(&openai, "gpt-5");
        assert_eq!(out["object"], "response");
        assert_eq!(out["status"], "completed");
        assert_eq!(out["output"][0]["type"], "message");
        assert_eq!(out["output"][0]["content"][0]["text"], "Hi");
        assert_eq!(out["usage"]["total_tokens"], 5);
    }

    #[test]
    fn test_stream_renderer_events() {
        let mut r = ResponsesStreamRenderer::new();
        let c1 = json!({"id":"x","model":"m","choices":[{"delta":{"content":"He"},"finish_reason":null}]});
        let frames = r.feed(&format!("data: {c1}"));
        let joined = frames.join("");
        assert!(joined.contains("event: response.created"));
        assert!(joined.contains("event: response.output_text.delta"));

        let c2 = json!({"id":"x","model":"m","choices":[{"delta":{},"finish_reason":"stop"}],
                        "usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}});
        let frames = r.feed(&format!("data: {c2}"));
        assert!(frames.join("").contains("event: response.completed"));
    }
}
