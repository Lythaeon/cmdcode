//! Ollama-native wire protocol (`/api/chat`, `/api/tags`).
//!
//! Serves clients that hardcode Ollama's endpoints. Distinct from Ollama's
//! OpenAI-compat layer (which our openai adapter already covers).

use crate::wire_format::{ChatCompletionRequest, FinishReason, OpenAiMessage};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Value};

/// Incoming `/api/chat` request.
#[derive(Debug, Clone, Deserialize)]
pub struct OllamaChatRequest {
    /// Model name.
    pub model: String,
    /// Conversation messages (OpenAI-compatible roles/content).
    #[serde(default)]
    pub messages: Vec<OpenAiMessage>,
    /// Whether to stream NDJSON chunks (default true in Ollama).
    #[serde(default = "default_true")]
    pub stream: bool,
    /// Tool definitions (Ollama uses the OpenAI tool shape).
    #[serde(default)]
    pub tools: Option<Value>,
    /// Sampler knobs (temperature, top_p, ...).
    #[serde(default)]
    pub options: Option<Value>,
}

fn default_true() -> bool {
    true
}

impl OllamaChatRequest {
    /// Convert to the internal OpenAI-format request.
    pub fn to_chat_completion(&self) -> ChatCompletionRequest {
        ChatCompletionRequest {
            model: Some(self.model.clone()),
            messages: self.messages.clone(),
            tools: self
                .tools
                .as_ref()
                .and_then(|t| serde_json::from_value(t.clone()).ok()),
            max_tokens: None, // Ollama has no default cap
            temperature: self.options_temperature(),
            stream: Some(self.stream),
            reasoning_effort: None,
            stream_options: None,
            frequency_penalty: None,
            presence_penalty: None,
            top_p: self.options_top_p(),
            stop: None,
            user: None,
            n: None,
            seed: None,
            response_format: None,
            logprobs: None,
            top_logprobs: None,
        }
    }

    fn options_field<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        self.options
            .as_ref()
            .and_then(|o| o.get(key))
            .cloned()
            .and_then(|v| serde_json::from_value(v).ok())
    }

    fn options_temperature(&self) -> Option<f64> {
        self.options_field("temperature")
    }

    fn options_top_p(&self) -> Option<f64> {
        self.options_field("top_p")
    }
}

/// Convert an internal OpenAI completion into an Ollama chat response
/// (single NDJSON object).
pub fn completion_to_ollama(openai: &Value, model: &str) -> Value {
    let choice = openai
        .pointer("/choices/0")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let message = choice.get("message").cloned().unwrap_or_else(|| json!({}));

    let mut msg = json!({"role": "assistant", "content": message.get("content").and_then(|c| c.as_str()).unwrap_or_default()});
    if let Some(calls) = message.get("tool_calls").and_then(|c| c.as_array()) {
        let ollama_calls: Vec<Value> = calls
            .iter()
            .map(|call| {
                let args_str = call
                    .pointer("/function/arguments")
                    .and_then(|a| a.as_str())
                    .unwrap_or("{}");
                json!({
                    "function": {
                        "name": call.pointer("/function/name").cloned().unwrap_or_else(|| json!("")),
                        "arguments": serde_json::from_str::<Value>(args_str)
                            .unwrap_or_else(|_| json!({})),
                    },
                })
            })
            .collect();
        msg["tool_calls"] = Value::Array(ollama_calls);
    }

    let prompt_count = openai
        .pointer("/usage/prompt_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let eval_count = openai
        .pointer("/usage/completion_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    json!({
        "model": model,
        "created_at": format!("{}Z", chrono_rfc3339_now()),
        "message": msg,
        "done": true,
        "prompt_eval_count": prompt_count,
        "eval_count": eval_count,
    })
}

/// RFC-3339 timestamp for `now` (seconds precision, no external crates).
fn chrono_rfc3339_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = secs / 86400;
    let rem = secs % 86400;
    let (y, m, d) = civil_from_days(days as i64);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe as i64 + era * 400;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Renders provider-emitted OpenAI SSE chunks into Ollama NDJSON chunks.
///
/// Each upstream delta becomes one line `{"message":{"content":"..."},"done":false}`;
/// the finish chunk becomes the terminal object with `"done":true` + counts.
#[derive(Debug, Default)]
pub struct OllamaStreamRenderer {
    model: String,
    finished: bool,
}

impl OllamaStreamRenderer {
    /// Create a renderer bound to the requested model name.
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            finished: false,
        }
    }

    /// Consume one OpenAI SSE payload, returning zero or more NDJSON lines
    /// (each terminated with `\n`).
    pub fn feed(&mut self, payload: &str) -> Vec<String> {
        let data = payload
            .trim()
            .strip_prefix("data:")
            .unwrap_or(payload)
            .trim();
        if data.is_empty() || data == "[DONE]" || self.finished {
            return Vec::new();
        }
        let Ok(chunk) = serde_json::from_str::<Value>(data) else {
            return Vec::new();
        };

        let mut out = Vec::new();
        let Some(choice) = chunk.pointer("/choices/0") else {
            return out;
        };
        let delta = choice.get("delta").cloned().unwrap_or_else(|| json!({}));

        let text = delta
            .get("content")
            .and_then(|c| c.as_str())
            .unwrap_or_default();
        if !text.is_empty() {
            out.push(format!(
                "{}\n",
                json!({
                    "model": self.model,
                    "created_at": format!("{}Z", chrono_rfc3339_now()),
                    "message": {"role": "assistant", "content": text},
                    "done": false,
                })
            ));
        }

        if let Some(reason) = choice
            .get("finish_reason")
            .and_then(|f| f.as_str())
            .map(str::to_string)
        {
            let finish_reason = FinishReason::from_upstream(&reason);
            let prompt = chunk
                .pointer("/usage/prompt_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let eval = chunk
                .pointer("/usage/completion_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            out.push(format!(
                "{}\n",
                json!({
                    "model": self.model,
                    "created_at": format!("{}Z", chrono_rfc3339_now()),
                    "message": {"role": "assistant", "content": ""},
                    "done_reason": finish_reason.to_string(),
                    "done": true,
                    "prompt_eval_count": prompt,
                    "eval_count": eval,
                })
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
    fn test_to_chat_completion() {
        let req: OllamaChatRequest = serde_json::from_value(json!({
            "model": "llama3",
            "messages": [{"role": "user", "content": "Hi"}],
            "stream": false,
            "options": {"temperature": 0.2}
        }))
        .unwrap();
        let cc = req.to_chat_completion();
        assert_eq!(cc.model.as_deref(), Some("llama3"));
        assert_eq!(cc.stream, Some(false));
        assert_eq!(cc.temperature, Some(0.2));
        assert_eq!(cc.messages[0].role, "user");
    }

    #[test]
    fn test_completion_to_ollama() {
        let openai = json!({
            "choices": [{
                "finish_reason": "stop",
                "message": {"role": "assistant", "content": "Hey"}
            }],
            "usage": {"prompt_tokens": 2, "completion_tokens": 1},
        });
        let out = completion_to_ollama(&openai, "llama3");
        assert_eq!(out["done"], true);
        assert_eq!(out["message"]["content"], "Hey");
        assert_eq!(out["prompt_eval_count"], 2);
        assert_eq!(out["eval_count"], 1);
    }

    #[test]
    fn test_stream_renderer() {
        let mut r = OllamaStreamRenderer::new("llama3");
        let c1 = json!({"id":"x","choices":[{"delta":{"content":"He"},"finish_reason":null}]});
        let lines = r.feed(&format!("data: {c1}"));
        assert_eq!(lines.len(), 1);
        let parsed: Value = serde_json::from_str(lines[0].trim()).unwrap();
        assert_eq!(parsed["message"]["content"], "He");
        assert_eq!(parsed["done"], false);

        let c2 = json!({"id":"x","choices":[{"delta":{},"finish_reason":"stop"}],
                        "usage":{"prompt_tokens":1,"completion_tokens":1}});
        let lines = r.feed(&format!("data: {c2}"));
        let parsed: Value = serde_json::from_str(lines[0].trim()).unwrap();
        assert_eq!(parsed["done"], true);
    }
}
