//! Generic OpenAI-compatible upstream adapter (pass-through).
//!
//! Works with any endpoint speaking the OpenAI chat completions protocol:
//! OpenAI, Azure OpenAI gateways, Ollama, vLLM, OpenRouter, Groq, etc.
//!
//! Configure with:
//! - `COMMAND_CODE_PROXY_PROVIDER=openai`
//! - `COMMAND_CODE_API_BASE=https://<host>` (base URL; `/chat/completions`
//!   is appended — include `/v1` in the base if the provider needs it)
//! - `COMMAND_CODE_UPSTREAM_API_KEY=<bearer token>`

use super::{Provider, RequestContext};
use cmdcode_core::auth::AuthManager;
use cmdcode_core::error::UpstreamError;
use crate::upstream::{LineOutcome, StreamState};

/// OpenAI-compatible pass-through provider.
#[derive(Clone)]
pub struct OpenAiProvider {
    /// Upstream base URL (e.g. `https://api.openai.com/v1`).
    pub base_url: String,
    /// Bearer token for upstream auth.
    pub api_key: Option<String>,
}

/// Prepend the taste section to the leading system message (or create one).
fn apply_taste(body: &mut serde_json::Value, taste: Option<&str>) {
    let Some(taste) = taste else { return };
    let Some(messages) = body.get_mut("messages").and_then(|m| m.as_array_mut()) else {
        return;
    };
    let system_prefix = format!("{taste}\n\n");
    // Find the first system message and prepend; otherwise insert one.
    if let Some(first_system) = messages.iter_mut().find(|m| {
        m.get("role").and_then(|r| r.as_str()) == Some("system")
    }) {
        match first_system.get_mut("content") {
            Some(serde_json::Value::String(content)) => {
                let merged = format!("{system_prefix}{content}");
                *content = merged;
            }
            _ => {
                first_system["content"] =
                    serde_json::Value::String(system_prefix.trim_end().into());
            }
        }
    } else {
        messages.insert(
            0,
            serde_json::json!({"role": "system", "content": taste}),
        );
    }
}

#[async_trait::async_trait]
impl Provider for OpenAiProvider {
    fn name(&self) -> &'static str {
        "openai"
    }

    fn endpoint(&self, _model: &str, _streaming: bool) -> String {
        format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
    }

    async fn headers(
        &self,
        _auth: &AuthManager,
        _cwd: &str,
    ) -> Result<Vec<(String, String)>, UpstreamError> {
        let mut headers = vec![
            ("Content-Type".to_string(), "application/json".to_string()),
            (
                "Authorization".to_string(),
                format!(
                    "Bearer {}",
                    self.api_key.as_deref().unwrap_or("not-needed")
                ),
            ),
        ];
        headers.shrink_to_fit();
        Ok(headers)
    }

    fn build_body(&self, ctx: &RequestContext<'_>) -> serde_json::Value {
        // Pass the original request through, swapping in the upstream model
        // name and injecting taste into the system message.
        let b = ctx.body;
        let mut body = serde_json::json!({
            "model": ctx.model.as_str(),
            "messages": b.messages,
            "stream": true,
        });
        if let Some(t) = &b.tools {
            body["tools"] = serde_json::to_value(t).unwrap_or(serde_json::Value::Null);
        }
        if let Some(v) = b.max_tokens {
            body["max_tokens"] = serde_json::json!(v);
        }
        if let Some(v) = b.temperature {
            body["temperature"] = serde_json::json!(v);
        }
        if let Some(e) = ctx.effort {
            body["reasoning_effort"] = serde_json::json!(e.as_str());
        }
        if let Some(v) = b.top_p {
            body["top_p"] = serde_json::json!(v);
        }
        if let Some(v) = b.frequency_penalty {
            body["frequency_penalty"] = serde_json::json!(v);
        }
        if let Some(v) = b.presence_penalty {
            body["presence_penalty"] = serde_json::json!(v);
        }
        if let Some(v) = &b.stop {
            body["stop"] = serde_json::to_value(v).unwrap_or(serde_json::Value::Null);
        }
        if let Some(v) = &b.user {
            body["user"] = serde_json::json!(v);
        }
        apply_taste(&mut body, ctx.taste_section.as_deref());
        body
    }

    fn translate_line<'a>(
        &self,
        line: &str,
        state: &mut StreamState<'a>,
    ) -> LineOutcome {
        openai_translate_line(line, state)
    }

    fn parse_non_streaming(
        &self,
        text: &str,
        _model: &str,
    ) -> Result<serde_json::Value, UpstreamError> {
        // Already OpenAI format — validate JSON and return.
        serde_json::from_str(text).map_err(|e| UpstreamError::HttpError {
            status: 502,
            body: format!("invalid upstream response: {e}"),
        })
    }

    fn is_auth_rejected(&self, status: u16) -> bool {
        status == 401
    }
}

/// Pass-through translation for OpenAI SSE lines.
///
/// OpenAI streams are already `data: {...}` SSE payloads; forward them
/// verbatim, tracking finish so `[DONE]` terminates cleanly.
pub fn openai_translate_line<'a>(
    line: &str,
    state: &mut StreamState<'a>,
) -> LineOutcome {
    let line = line.trim();
    if line.is_empty() {
        return LineOutcome::Skip;
    }
    if !line.starts_with("data:") {
        // Comments / keep-alives / non-SSE noise
        return LineOutcome::Skip;
    }
    let payload = line.trim_start_matches("data:").trim();
    if payload == "[DONE]" {
        state.finish_seen = true;
        return LineOutcome::EmitAndStop("data: [DONE]\n\n".to_string());
    }
    // Forward verbatim as a complete SSE frame.
    LineOutcome::Emit(format!("data: {payload}\n\n"))
}
