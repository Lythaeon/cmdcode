//! Upstream provider adapters.
//!
//! The proxy accepts OpenAI-compatible chat completion requests and forwards
//! them to one of several upstream providers. Each provider translates the
//! normalized request into its own wire format and translates responses back.
//!
//! Select via `COMMAND_CODE_PROXY_PROVIDER`:
//! - `command-code` (default): Command Code `/alpha/generate` NDJSON protocol
//! - `openai`: pass-through for any OpenAI-compatible endpoint

pub mod commandcode;
pub mod openai;

use cmdcode_core::auth::AuthManager;
use cmdcode_core::error::UpstreamError;
use cmdcode_core::types::{Effort, ModelId};
use cmdcode_core::wire_format::ChatCompletionRequest;
use std::sync::Arc;

/// Everything a provider needs to build its request body.
pub struct RequestContext<'a> {
    /// Requested model (already resolved/validated).
    pub model: &'a ModelId,
    /// Original OpenAI-compatible request.
    pub body: &'a ChatCompletionRequest,
    /// Parsed reasoning effort, if any.
    pub effort: Option<Effort>,
    /// Current working directory (for taste/config context).
    pub cwd: &'a str,
    /// Rendered `<taste>` section when taste learning is enabled and the
    /// provider wants it injected; `None` otherwise. Providers decide where
    /// it goes in their wire format.
    pub taste_section: Option<String>,
}

/// One upstream provider adapter.
#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    /// Adapter identifier (also the `COMMAND_CODE_PROXY_PROVIDER` value).
    fn name(&self) -> &'static str;

    /// Full endpoint URL for chat completions.
    fn endpoint(&self, base_url: &str) -> String;

    /// Identity/auth headers for an upstream call.
    async fn headers(
        &self,
        auth: &AuthManager,
        cwd: &str,
    ) -> Result<Vec<(String, String)>, UpstreamError>;

    /// Build the provider-specific request body.
    fn build_body(&self, ctx: &RequestContext<'_>) -> serde_json::Value;

    /// Translate one line of the upstream response stream into downstream
    /// SSE payload(s).
    fn translate_line<'a>(
        &self,
        line: &str,
        state: &mut crate::upstream::StreamState<'a>,
    ) -> crate::upstream::LineOutcome;

    /// Parse a non-streaming response body into an OpenAI completion JSON.
    fn parse_non_streaming(
        &self,
        text: &str,
        model: &str,
    ) -> Result<serde_json::Value, UpstreamError>;

    /// Whether an HTTP status indicates credential rejection.
    fn is_auth_rejected(&self, status: u16) -> bool;

    /// Handle credential rejection. Returns the rotated account name if
    /// credentials were refreshed (caller may retry immediately).
    async fn on_auth_rejected(&self, _auth: &AuthManager) -> Option<String> {
        None
    }
}

/// Build the configured provider adapter.
pub fn from_config(config: &cmdcode_core::config::ProxyConfig, auth: Arc<AuthManager>) -> Box<dyn Provider> {
    match config.provider.as_str() {
        "openai" => Box::new(openai::OpenAiProvider {
            api_key: config.provider_api_key.clone(),
        }),
        // Default and explicit fallback
        _ => Box::new(commandcode::CommandCodeProvider { auth }),
    }
}
