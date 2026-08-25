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
use crate::upstream::{LineOutcome, StreamState};
use cmdcode_core::auth::AuthManager;
use cmdcode_core::error::UpstreamError;

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
    if let Some(first_system) = messages
        .iter_mut()
        .find(|m| m.get("role").and_then(|r| r.as_str()) == Some("system"))
    {
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
        messages.insert(0, serde_json::json!({"role": "system", "content": taste}));
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
                format!("Bearer {}", self.api_key.as_deref().unwrap_or("not-needed")),
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

    fn translate_line<'a>(&self, line: &str, state: &mut StreamState<'a>) -> LineOutcome {
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
/// OpenAI streams are already `data: {...}` SSE payloads. We forward real
/// frames verbatim, tracking finish so `[DONE]` terminates cleanly — but we
/// must validate each `data:` payload first. Upstreams occasionally interleave
/// non-JSON `data:` lines (debug logs, keep-alives, or a raw request/error
/// echo such as `data: gemini-upstream saw key=... sys='...' msg='...'`).
/// Forwarding those verbatim would tunnel arbitrary upstream output to the
/// client as if it were a model completion — the stream "echoes the prompt
/// back and stops". Only well-formed OpenAI SSE frames (`choices`, `error`,
/// or `[DONE]`) are emitted; everything else is dropped.
pub fn openai_translate_line<'a>(line: &str, state: &mut StreamState<'a>) -> LineOutcome {
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
    let Ok(frame) = serde_json::from_str::<serde_json::Value>(payload) else {
        // Not valid JSON: an upstream log/echo line (e.g. a `data:`-prefixed
        // debug message). Passing it through would surface it to the client as
        // a completion, so drop it instead.
        tracing::warn!(
            raw_line = %payload.chars().take(200).collect::<String>(),
            "skipped non-JSON upstream SSE frame"
        );
        return LineOutcome::Skip;
    };
    // A real OpenAI chunk carries `choices`; an in-stream error carries
    // `error`. Anything else is upstream noise we must not present as output.
    if frame.get("choices").is_some() || frame.get("error").is_some() {
        LineOutcome::Emit(format!("data: {payload}\n\n"))
    } else {
        tracing::warn!(
            raw_line = %payload.chars().take(200).collect::<String>(),
            "skipped upstream SSE frame without choices or error"
        );
        LineOutcome::Skip
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::upstream::StreamState;

    fn state<'a>(id: &'a str, model: &'a str) -> StreamState<'a> {
        StreamState {
            completion_id: id,
            created: 0,
            model,
            tool_index: 0,
            skipped: 0,
            finish_seen: false,
            tool_parts: std::collections::HashMap::new(),
            skipped_by_type: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn forwards_valid_chunk_with_choices() {
        let mut s = state("c", "m");
        match openai_translate_line(r#"data: {"choices":[{"delta":{"content":"hi"}}]}"#, &mut s) {
            LineOutcome::Emit(p) => assert!(p.contains("\"content\":\"hi\"")),
            _ => panic!("expected Emit"),
        }
    }

    #[test]
    fn forwards_in_stream_error_frame() {
        let mut s = state("c", "m");
        match openai_translate_line(
            r#"data: {"error":{"message":"boom","type":"upstream_error"}}"#,
            &mut s,
        ) {
            LineOutcome::Emit(p) => assert!(p.contains("\"error\"")),
            _ => panic!("expected Emit"),
        }
    }

    #[test]
    fn skips_non_json_upstream_log_echo() {
        // The reported bug: a `data:`-prefixed debug/echo line from the
        // upstream must NOT be tunneled to the client as a completion.
        let mut s = state("c", "m");
        assert_eq!(
            openai_translate_line(
                "data: gemini-upstream saw key=AIza-test..., sys='...' msg='check out this project'",
                &mut s,
            ),
            LineOutcome::Skip
        );
    }

    #[test]
    fn skips_json_frame_without_choices_or_error() {
        let mut s = state("c", "m");
        assert_eq!(
            openai_translate_line(r#"data: {"some":"noise"}"#, &mut s),
            LineOutcome::Skip
        );
    }

    #[test]
    fn done_terminates_stream() {
        let mut s = state("c", "m");
        match openai_translate_line("data: [DONE]", &mut s) {
            LineOutcome::EmitAndStop(p) => {
                assert!(p.contains("[DONE]"));
                assert!(s.finish_seen);
            }
            _ => panic!("expected EmitAndStop"),
        }
    }
}
