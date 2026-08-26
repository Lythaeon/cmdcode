//! Command Code upstream adapter (`/alpha/generate` NDJSON protocol).

use super::{Provider, RequestContext};
use crate::upstream::{build_config, extract_system, is_auth_rejected, LineOutcome, StreamState};
use cmdcode_core::auth::AuthManager;
use cmdcode_core::error::UpstreamError;
use cmdcode_core::types::FinishReason;
use cmdcode_core::wire_format::{
    build_completion, wire_messages, wire_tools, CcUsage, UpstreamEvent,
};
use std::sync::Arc;

/// Command Code provider — CLI-fingerprint headers, vault auth with rotation.
pub struct CommandCodeProvider {
    /// Credential manager (vault + auth.json) with account rotation.
    pub auth: Arc<AuthManager>,
    /// Upstream base URL (e.g. `https://api.commandcode.ai`).
    pub base_url: String,
    /// Whether this entry serves taste learning requests.
    pub learning: bool,
}

#[async_trait::async_trait]
impl Provider for CommandCodeProvider {
    fn name(&self) -> &'static str {
        "command-code"
    }

    fn endpoint(&self, _model: &str, _streaming: bool) -> String {
        format!("{}/alpha/generate", self.base_url.trim_end_matches('/'))
    }

    async fn headers(
        &self,
        auth: &AuthManager,
        cwd: &str,
    ) -> Result<Vec<(String, String)>, UpstreamError> {
        auth.build_headers(cwd)
            .await
            .map(|h| h.into_iter().collect())
            .map_err(|e| UpstreamError::Io(std::io::Error::other(e.to_string())))
    }

    fn build_body(&self, ctx: &RequestContext<'_>) -> serde_json::Value {
        let wire_msgs = wire_messages(&ctx.body.messages);
        let wire_tools = wire_tools(ctx.body.tools.as_deref().unwrap_or_default());
        let max_tokens = ctx.body.max_tokens.unwrap_or(64000);

        let mut params = serde_json::json!({
            "model": ctx.model.as_str(),
            "messages": wire_msgs,
            "tools": wire_tools,
            "max_tokens": max_tokens,
            "stream": true,
        });
        let Some(params_obj) = params.as_object_mut() else {
            return params;
        };

        if let Some(system) = extract_system(&ctx.body.messages) {
            // Prepend the taste section if provided (taste learning enabled).
            // Mirrors the CLI: always rendered, with a "no preferences yet"
            // block when empty so the agent knows learning is active.
            let system = match &ctx.taste_section {
                Some(taste) => format!("{taste}\n\n{system}"),
                None => system,
            };
            params_obj.insert("system".into(), serde_json::Value::String(system));
        }
        if let Some(t) = ctx.body.temperature {
            params_obj.insert("temperature".into(), serde_json::json!(t));
        }
        if let Some(e) = ctx.effort {
            params_obj.insert("reasoning_effort".into(), serde_json::json!(e.as_str()));
        }
        if let Some(p) = ctx.body.top_p {
            params_obj.insert("top_p".into(), serde_json::json!(p));
        }
        if let Some(fp) = ctx.body.frequency_penalty {
            params_obj.insert("frequency_penalty".into(), serde_json::json!(fp));
        }
        if let Some(pp) = ctx.body.presence_penalty {
            params_obj.insert("presence_penalty".into(), serde_json::json!(pp));
        }
        if let Some(stop) = &ctx.body.stop {
            params_obj.insert(
                "stop".into(),
                serde_json::to_value(stop).unwrap_or_default(),
            );
        }
        if let Some(user) = &ctx.body.user {
            params_obj.insert("user".into(), serde_json::json!(user));
        }

        serde_json::json!({
            "config": build_config(ctx.cwd),
            "memory": null,
            "taste": null,
            "skills": null,
            "permissionMode": "standard",
            "mode": "agent",
            "params": params,
        })
    }

    fn translate_line<'a>(&self, line: &str, state: &mut StreamState<'a>) -> LineOutcome {
        crate::upstream::translate_line(line, state)
    }

    fn parse_non_streaming(
        &self,
        text: &str,
        model: &str,
    ) -> Result<serde_json::Value, UpstreamError> {
        let mut text_parts = Vec::new();
        let mut reasoning_parts = Vec::new();
        let mut tool_calls = Vec::new();
        let mut usage = CcUsage::default();
        let mut finish_reason = FinishReason::Stop;
        let mut saw_finish = false;

        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(evt) = serde_json::from_str::<UpstreamEvent>(line) {
                match evt.event_type.as_str() {
                    "text-delta" => {
                        if let Some(t) = evt.text {
                            text_parts.push(t);
                        }
                    }
                    "reasoning-delta" => {
                        if let Some(t) = evt.text {
                            reasoning_parts.push(t);
                        }
                    }
                    "tool-call" => {
                        tool_calls.push((
                            evt.tool_call_id.unwrap_or_default(),
                            evt.tool_name.unwrap_or_default(),
                            evt.input.unwrap_or(serde_json::Value::Null),
                        ));
                    }
                    "finish" => {
                        saw_finish = true;
                        if let Some(u) = evt.total_usage {
                            usage.input_tokens = u.input_tokens.unwrap_or(0);
                            usage.output_tokens = u.output_tokens.unwrap_or(0);
                            if let Some(d) = u.input_token_details {
                                usage.cache_read_tokens = d.cache_read_tokens.unwrap_or(0);
                            }
                        }
                        let raw = evt
                            .raw_finish_reason
                            .as_deref()
                            .or(evt.finish_reason.as_deref())
                            .unwrap_or("stop");
                        finish_reason = FinishReason::from_upstream(raw);
                    }
                    "error" => {
                        return Err(UpstreamError::HttpError {
                            status: 502,
                            body: evt
                                .error
                                .and_then(|e| e.message)
                                .unwrap_or_else(|| "stream error".into()),
                        });
                    }
                    _ => {}
                }
            }
        }

        if !saw_finish {
            return Err(UpstreamError::HttpError {
                status: 502,
                body: "upstream ended without finish event".into(),
            });
        }

        serde_json::to_value(build_completion(
            model,
            &text_parts.join(""),
            &reasoning_parts.join(""),
            &tool_calls,
            finish_reason,
            &usage,
        ))
        .map_err(|e| UpstreamError::HttpError {
            status: 502,
            body: format!("response serialization: {e}"),
        })
    }

    fn is_auth_rejected(&self, status: u16) -> bool {
        is_auth_rejected(status)
    }

    fn should_rotate(&self, status: u16, error_body: &str) -> bool {
        // Credit/limit exhaustion comes back as 400 BAD_REQUEST with a
        // message — rotate to another account just like an auth rejection.
        if is_auth_rejected(status) {
            return true;
        }
        let lower = error_body.to_lowercase();
        lower.contains("insufficient credits")
            || (lower.contains("credit")
                && (lower.contains("exhaust") || lower.contains("purchase more")))
    }

    async fn on_auth_rejected(&self, auth: &AuthManager) -> Option<String> {
        auth.on_auth_rejected().await
    }
}
