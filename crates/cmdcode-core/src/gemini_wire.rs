//! Google Gemini wire protocol (`:generateContent` / `:streamGenerateContent`).
//!
//! Bridges Gemini's contents/parts representation onto the proxy's internal
//! OpenAI-format request/response types so any configured upstream provider
//! can serve Gemini clients (Gemini CLI, google-genai SDKs).

use crate::wire_format::{ChatCompletionRequest, OpenAiFunctionRef, OpenAiMessage, OpenAiTool, OpenAiToolCall};
use serde::Deserialize;
use serde_json::{json, Value};

/// One content part; only `text`, `functionCall` and `functionResponse` are
/// interpreted, everything else passes through as raw JSON.
#[derive(Debug, Clone, Deserialize)]
pub struct GeminiPart {
    /// Text part.
    #[serde(default)]
    pub text: Option<String>,
    /// Model-initiated tool call.
    #[serde(rename = "functionCall", default)]
    pub function_call: Option<GeminiFunctionCall>,
    /// Client's tool result.
    #[serde(rename = "functionResponse", default)]
    pub function_response: Option<GeminiFunctionResponse>,
    /// Any other part (inlineData etc.) preserved raw.
    #[serde(flatten)]
    pub extra: Value,
}

/// A model-initiated tool call part.
#[derive(Debug, Clone, Deserialize)]
pub struct GeminiFunctionCall {
    /// Tool name.
    pub name: String,
    /// Arguments object.
    #[serde(default)]
    pub args: Value,
}

/// A client's tool result part.
#[derive(Debug, Clone, Deserialize)]
pub struct GeminiFunctionResponse {
    /// Tool name being answered.
    pub name: String,
    /// Result payload object.
    #[serde(default)]
    pub response: Value,
}

/// One turn; roles are `user` and `model`.
#[derive(Debug, Clone, Deserialize)]
pub struct GeminiContent {
    /// `user` or `model`.
    #[serde(default)]
    pub role: String,
    /// Content parts for this turn.
    #[serde(default)]
    pub parts: Vec<GeminiPart>,
}

/// Flat function declaration inside a `tools` entry.
/// Flat function declaration inside a `tools` entry.
#[derive(Debug, Clone, Deserialize)]
pub struct GeminiFunctionDeclaration {
    /// Tool name.
    pub name: String,
    #[serde(default)]
    /// What the tool does.
    pub description: Option<String>,
    #[serde(rename = "parameters", default)]
    /// JSON Schema for parameters.
    pub parameters: Value,
}

/// Generation config (sampling knobs).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct GeminiGenerationConfig {
    /// Sampling temperature.
    #[serde(rename = "temperature", default)]
    pub temperature: Option<f64>,
    /// Nucleus sampling threshold.
    #[serde(rename = "topP", default)]
    pub top_p: Option<f64>,
    /// Output token cap.
    #[serde(rename = "maxOutputTokens", default)]
    pub max_output_tokens: Option<u32>,
    /// Stop sequences.
    #[serde(rename = "stopSequences", default)]
    pub stop_sequences: Option<Vec<String>>,
}

/// Incoming `{model}:generateContent` request body.
#[derive(Debug, Clone, Deserialize)]
pub struct GeminiRequest {
    /// Conversation turns in order.
    #[serde(default)]
    pub contents: Vec<GeminiContent>,
    /// System instruction (parts), rendered as a leading system message.
    #[serde(rename = "systemInstruction", alias = "system_instruction", default)]
    pub system_instruction: Option<Value>,
    /// Tools; each entry holds a `functionDeclarations` list.
    #[serde(default)]
    pub tools: Option<Vec<Value>>,
    /// Sampling configuration.
    #[serde(rename = "generationConfig", default)]
    pub generation_config: GeminiGenerationConfig,
}

/// Extract text from a systemInstruction value (object with parts or string).
fn system_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Object(_) => v
            .get("parts")
            .and_then(|p| p.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|part| part.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default(),
        _ => String::new(),
    }
}

fn parts_to_openai_content(parts: &[GeminiPart]) -> Value {
    // Pure-text turns become plain strings; anything else stays structured.
    let all_text = parts.len() == 1 && parts[0].text.is_some();
    if all_text {
        return json!(parts[0].text.clone().unwrap_or_default());
    }
    let mut out: Vec<Value> = Vec::new();
    for p in parts {
        if let Some(text) = &p.text {
            out.push(json!({"type": "text", "text": text}));
        }
        // functionCall/functionResponse are handled at the conversion level;
        // other parts pass through raw.
    }
    json!(out)
}

impl GeminiRequest {
    /// Convert to the internal OpenAI-format request.
    pub fn to_chat_completion(&self) -> ChatCompletionRequest {
        let mut messages: Vec<OpenAiMessage> = Vec::new();

        if let Some(si) = &self.system_instruction {
            let text = system_text(si);
            if !text.is_empty() {
                messages.push(OpenAiMessage {
                    role: "system".into(),
                    content: Some(json!(text)),
                    tool_calls: None,
                    tool_call_id: None,
                });
            }
        }

        for content in &self.contents {
            // Gemini "model" == OpenAI "assistant".
            let role = if content.role == "model" {
                "assistant"
            } else {
                "user"
            };

            let has_responses = content
                .parts
                .iter()
                .any(|p| p.function_response.is_some());

            if has_responses && role == "user" {
                // Tool results -> one OpenAI tool message per part.
                for p in &content.parts {
                    if let Some(fr) = &p.function_response {
                        messages.push(OpenAiMessage {
                            role: "tool".into(),
                            content: Some(json!(fr.response.to_string())),
                            tool_calls: None,
                            tool_call_id: Some(fr.name.clone()),
                        });
                    } else if let Some(text) = &p.text {
                        messages.push(OpenAiMessage {
                            role: "user".into(),
                            content: Some(json!(text)),
                            tool_calls: None,
                            tool_call_id: None,
                        });
                    }
                }
                continue;
            }

            let mut tool_calls: Vec<OpenAiToolCall> = Vec::new();
            for p in &content.parts {
                if let Some(fc) = &p.function_call {
                    tool_calls.push(OpenAiToolCall {
                        id: Some(format!("call_{}", fc.name)),
                        function: Some(OpenAiFunctionRef {
                            name: Some(fc.name.clone()),
                            arguments: Some(
                                serde_json::to_string(&fc.args).unwrap_or_default(),
                            ),
                        }),
                    });
                }
            }

            let msg_content = if tool_calls.is_empty() {
                Some(parts_to_openai_content(&content.parts))
            } else {
                None
            };
            messages.push(OpenAiMessage {
                role: role.into(),
                content: msg_content,
                tool_calls: if tool_calls.is_empty() {
                    None
                } else {
                    Some(tool_calls)
                },
                tool_call_id: None,
            });
        }

        let tools: Option<Vec<OpenAiTool>> = self.tools.as_ref().map(|entries| {
            entries
                .iter()
                .filter_map(|e| e.get("functionDeclarations"))
                .filter_map(|d| d.as_array())
                .flatten()
                .map(|decl| {
                    let decl: GeminiFunctionDeclaration = serde_json::from_value(decl.clone())
                        .unwrap_or_else(|_| GeminiFunctionDeclaration {
                            name: String::new(),
                            description: None,
                            parameters: json!({"type": "object"}),
                        });
                    OpenAiTool {
                        tool_type: "function".into(),
                        function: Some(crate::wire_format::OpenAiFunction {
                            name: decl.name,
                            description: decl.description,
                            parameters: if decl.parameters.is_null() {
                                None
                            } else {
                                Some(decl.parameters)
                            },
                        }),
                        name: None,
                        description: None,
                        input_schema: None,
                        parameters: None,
                    }
                })
                .collect()
        });

        ChatCompletionRequest {
            model: None, // supplied by the URL path
            messages,
            tools,
            max_tokens: self.generation_config.max_output_tokens,
            temperature: self.generation_config.temperature,
            stream: None, // decided by which endpoint was hit
            reasoning_effort: None,
            stream_options: None,
            frequency_penalty: None,
            presence_penalty: None,
            top_p: self.generation_config.top_p,
            stop: self.generation_config.stop_sequences.clone(),
            user: None,
            n: None,
            seed: None,
            response_format: None,
            logprobs: None,
            top_logprobs: None,
        }
    }
}

/// Map an OpenAI finish reason to a Gemini finishReason.
pub fn finish_reason_from_openai(reason: &str) -> &'static str {
    match reason {
        "length" => "MAX_TOKENS",
        "tool_calls" | "function_call" => "STOP",
        "content_filter" => "SAFETY",
        _ => "STOP",
    }
}

/// Convert an internal OpenAI completion JSON into a Gemini response.
pub fn completion_to_gemini(openai: &Value, model: &str) -> Value {
    let choice = openai
        .pointer("/choices/0")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let message = choice.get("message").cloned().unwrap_or_else(|| json!({}));

    let mut parts: Vec<Value> = Vec::new();
    if let Some(reasoning) = message
        .get("reasoning_content")
        .and_then(|r| r.as_str())
        .filter(|s| !s.is_empty())
    {
        parts.push(json!({"text": format!("[thinking] {reasoning}")}));
    }
    if let Some(calls) = message.get("tool_calls").and_then(|c| c.as_array()) {
        for call in calls {
            let args_str = call
                .pointer("/function/arguments")
                .and_then(|a| a.as_str())
                .unwrap_or("{}");
            let args: Value = serde_json::from_str(args_str).unwrap_or_else(|_| json!({}));
            parts.push(json!({
                "functionCall": {
                    "name": call.pointer("/function/name").cloned().unwrap_or_else(|| json!("")),
                    "args": args,
                },
            }));
        }
    }
    let text = message
        .get("content")
        .and_then(|c| c.as_str())
        .unwrap_or_default();
    if !text.is_empty() || parts.is_empty() {
        parts.push(json!({"text": text}));
    }

    let finish_reason = choice
        .get("finish_reason")
        .and_then(|f| f.as_str())
        .map(finish_reason_from_openai)
        .unwrap_or("STOP");

    let input = openai
        .pointer("/usage/prompt_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let output = openai
        .pointer("/usage/completion_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    json!({
        "candidates": [{
            "content": {"role": "model", "parts": parts},
            "finishReason": finish_reason,
            "index": 0,
        }],
        "usageMetadata": {
            "promptTokenCount": input,
            "candidatesTokenCount": output,
            "totalTokenCount": input + output,
        },
        "modelVersion": model,
    })
}

/// Renders provider-emitted OpenAI SSE chunks into Gemini streaming chunks
/// (`data: {candidates:[...]}` lines, matching `:streamGenerateContent`).
#[derive(Debug, Default)]
pub struct GeminiStreamRenderer {
    started: bool,
    finished: bool,
}

impl GeminiStreamRenderer {
    /// Create a fresh renderer for one streamed response.
    pub fn new() -> Self {
        Self::default()
    }

    /// Consume one OpenAI SSE payload, returning zero or more Gemini frames.
    pub fn feed(&mut self, payload: &str) -> Vec<String> {
        let data = payload.trim().strip_prefix("data:").unwrap_or(payload).trim();
        if data.is_empty() || data == "[DONE]" || self.finished {
            return Vec::new();
        }
        let Ok(chunk) = serde_json::from_str::<Value>(data) else {
            return Vec::new();
        };

        if !self.started {
            self.started = true;
        }

        let mut text_parts: Vec<&str> = Vec::new();
        let mut tool_parts: Vec<Value> = Vec::new();
        let mut finish: Option<&str> = None;

        if let Some(choice) = chunk.pointer("/choices/0") {
            if let Some(delta) = choice.get("delta") {
                if let Some(t) = delta.get("content").and_then(|c| c.as_str()) {
                    if !t.is_empty() {
                        text_parts.push(t);
                    }
                }
                if let Some(calls) = delta.get("tool_calls").and_then(|c| c.as_array()) {
                    for call in calls {
                        if let Some(name) = call.pointer("/function/name").and_then(|n| n.as_str())
                        {
                            tool_parts.push(json!({
                                "functionCall": {"name": name, "args": {}},
                            }));
                        }
                        if let Some(args) =
                            call.pointer("/function/arguments").and_then(|a| a.as_str())
                        {
                            if !args.is_empty() {
                                text_parts.push(args);
                            }
                        }
                    }
                }
            }
            finish = choice.get("finish_reason").and_then(|f| f.as_str());
        }

        let mut candidates_meta = json!([]);
        let _ = &mut candidates_meta;

        let mut parts: Vec<Value> = text_parts
            .into_iter()
            .map(|t| json!({"text": t}))
            .collect();
        parts.extend(tool_parts);

        if parts.is_empty() && finish.is_none() {
            // Pure keep-alive chunk (no content change) — skip.
            return Vec::new();
        }

        let mut frame = json!({
            "candidates": [{
                "content": {"role": "model", "parts": parts},
                "index": 0,
            }],
        });

        if let Some(reason) = finish {
            frame["candidates"][0]["finishReason"] =
                json!(finish_reason_from_openai(reason));
            if let Some(u) = chunk.get("usage") {
                let input = u.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                let output = u
                    .get("completion_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                frame["usageMetadata"] = json!({
                    "promptTokenCount": input,
                    "candidatesTokenCount": output,
                    "totalTokenCount": input + output,
                });
            }
            self.finished = true;
        }

        vec![format!("data: {}\n\n", frame)]
    }
}

pub use crate::wire_format::build_completion as _build_completion_alias;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn sample_request() -> (GeminiRequest, String) {
        let req: GeminiRequest = serde_json::from_value(json!({
            "contents": [
                {"role": "user", "parts": [{"text": "Weather in Paris?"}]},
                {"role": "model", "parts": [
                    {"functionCall": {"name": "get_weather", "args": {"city": "Paris"}}}
                ]},
                {"role": "user", "parts": [
                    {"functionResponse": {"name": "get_weather",
                     "response": {"temp": 22}}}
                ]}
            ],
            "systemInstruction": {"parts": [{"text": "Be terse."}]},
            "tools": [{"functionDeclarations": [{
                "name": "get_weather",
                "description": "Get weather",
                "parameters": {"type": "object"}
            }]}],
            "generationConfig": {"temperature": 0.5, "maxOutputTokens": 256}
        }))
        .unwrap();
        (req, "gemini-2.0-flash".into())
    }

    #[test]
    fn test_to_chat_completion() {
        let (req, _) = sample_request();
        let cc = req.to_chat_completion();
        assert_eq!(cc.messages[0].role, "system");
        assert_eq!(cc.temperature, Some(0.5));
        assert_eq!(cc.max_tokens, Some(256));
        // user -> assistant(tool_calls) -> tool(result)
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
    fn test_completion_to_gemini() {
        let openai = json!({
            "choices": [{
                "finish_reason": "stop",
                "message": {"role": "assistant", "content": "22C"}
            }],
            "usage": {"prompt_tokens": 7, "completion_tokens": 3},
        });
        let out = completion_to_gemini(&openai, "gemini-2.0-flash");
        assert_eq!(out["candidates"][0]["content"]["role"], "model");
        assert_eq!(out["candidates"][0]["content"]["parts"][0]["text"], "22C");
        assert_eq!(out["candidates"][0]["finishReason"], "STOP");
        assert_eq!(out["usageMetadata"]["totalTokenCount"], 10);
    }

    #[test]
    fn test_stream_renderer() {
        let mut r = GeminiStreamRenderer::new();
        let c1 = json!({"id":"x","model":"m","choices":[{"delta":{"content":"He"},"finish_reason":null}]});
        let frames = r.feed(&format!("data: {c1}"));
        assert_eq!(frames.len(), 1);
        assert!(frames[0].starts_with("data: "));
        let parsed: Value = serde_json::from_str(frames[0].trim_start_matches("data: ").trim())
            .unwrap();
        assert_eq!(
            parsed["candidates"][0]["content"]["parts"][0]["text"],
            "He"
        );

        let c2 = json!({"choices":[{"delta":{},"finish_reason":"stop"}],
                        "usage":{"prompt_tokens":4,"completion_tokens":2}});
        let frames = r.feed(&format!("data: {c2}"));
        let parsed: Value =
            serde_json::from_str(frames[0].trim_start_matches("data: ").trim()).unwrap();
        assert_eq!(parsed["candidates"][0]["finishReason"], "STOP");
        assert_eq!(parsed["usageMetadata"]["totalTokenCount"], 6);
        // [DONE] produces nothing further
        assert!(r.feed("data: [DONE]").is_empty());
    }
}
