//! Upstream provider adapters.
//!
//! The proxy accepts OpenAI-compatible chat completion requests and forwards
//! them to one of several upstream providers. Each provider translates the
//! normalized request into its own wire format and translates responses back.
//!
//! Select via `COMMAND_CODE_PROXY_PROVIDER`:
//! - `command-code` (default): Command Code `/alpha/generate` NDJSON protocol
//! - `openai`: pass-through for any OpenAI-compatible endpoint

pub mod anthropic;
pub mod commandcode;
pub mod gemini;
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

    /// Full endpoint URL for chat completions. `streaming` selects the
    /// upstream's streaming route when a provider distinguishes them.
    fn endpoint(&self, model: &str, streaming: bool) -> String;

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

    /// Whether an upstream failure indicates the credential/account can no
    /// longer serve requests (auth rejected OR credit/limit exhaustion) and
    /// rotation should be attempted. Receives the error body for
    /// message-based detection (e.g. "insufficient credits").
    fn should_rotate(&self, status: u16, error_body: &str) -> bool {
        let _ = error_body;
        self.is_auth_rejected(status)
    }

    /// Whether an HTTP status indicates credential rejection.
    fn is_auth_rejected(&self, status: u16) -> bool;

    /// Handle credential rejection. Returns the rotated account name if
    /// credentials were refreshed (caller may retry immediately).
    async fn on_auth_rejected(&self, _auth: &AuthManager) -> Option<String> {
        None
    }
}

/// Stub used when every declared provider is disabled: serves a clear 503.
pub struct NoEnabledProvider;

#[async_trait::async_trait]
impl Provider for NoEnabledProvider {
    fn name(&self) -> &'static str {
        "none"
    }

    fn endpoint(&self, _model: &str, _streaming: bool) -> String {
        "http://cmdcode.invalid/disabled".into()
    }

    async fn headers(
        &self,
        _auth: &AuthManager,
        _cwd: &str,
    ) -> Result<Vec<(String, String)>, UpstreamError> {
        Ok(Vec::new())
    }

    fn build_body(&self, _ctx: &RequestContext<'_>) -> serde_json::Value {
        serde_json::json!({})
    }

    fn translate_line<'a>(
        &self,
        _line: &str,
        _state: &mut crate::upstream::StreamState<'a>,
    ) -> crate::upstream::LineOutcome {
        crate::upstream::LineOutcome::Skip
    }

    fn parse_non_streaming(
        &self,
        _text: &str,
        _model: &str,
    ) -> Result<serde_json::Value, UpstreamError> {
        Err(UpstreamError::HttpError {
            status: 503,
            body: "all providers are disabled".into(),
        })
    }

    fn is_auth_rejected(&self, _status: u16) -> bool {
        false
    }
}

/// Routes each request to the provider that declares its model.
///
/// Built from the opencode-style `providers.json` when present; otherwise a
/// single adapter synthesized from environment variables (back-compat).
pub struct ProviderRouter {
    /// Provider used when no model mapping matches (first declared entry).
    pub default: Arc<dyn Provider>,
    /// Model id -> provider that serves it.
    pub by_model: std::collections::HashMap<String, Arc<dyn Provider>>,
    /// Aggregated model list across all providers (`/v1/models`).
    pub models: Vec<serde_json::Value>,
    /// True when every declared entry was disabled — requests get a fast 503.
    pub all_disabled: bool,
    /// Config file path this router was built from (for hot reload), when
    /// a declarative config was used.
    pub source_path: Option<std::path::PathBuf>,
    /// mtime of the config at build time.
    pub source_mtime: Option<std::time::SystemTime>,
}

/// Shared, hot-reloadable router handle.
///
/// `reload_if_changed` stats the declarative config and swaps the router
/// in-place when it changed, so provider edits apply on the next request
/// without restarting the proxy. Env-only setups never reload (nothing to
/// watch).
pub struct RouterHandle {
    inner: tokio::sync::RwLock<Arc<ProviderRouter>>,
    config: Arc<cmdcode_core::config::ProxyConfig>,
    auth: Arc<AuthManager>,
}

impl RouterHandle {
    /// Wrap an initial router together with the inputs needed to rebuild it.
    pub fn new(
        router: ProviderRouter,
        config: Arc<cmdcode_core::config::ProxyConfig>,
        auth: Arc<AuthManager>,
    ) -> Self {
        Self {
            inner: tokio::sync::RwLock::new(Arc::new(router)),
            config,
            auth,
        }
    }

    /// Current router snapshot.
    pub async fn get(&self) -> Arc<ProviderRouter> {
        self.inner.read().await.clone()
    }

    /// Rebuild the router when the config file's mtime changed since the
    /// last build. Cheap stat per call; safe to run on every request.
    pub async fn reload_if_changed(&self) {
        let current = self.get().await;
        let Some(path) = &current.source_path else {
            return; // env-only setup — nothing to watch
        };
        let mtime = std::fs::metadata(path)
            .and_then(|m| m.modified())
            .ok();
        if mtime == current.source_mtime {
            return;
        }
        // A broken/missing file must NOT degrade to the env fallback here:
        // retain the last-good router so traffic keeps flowing.
        match ProviderRouter::try_build_declared(&self.config, self.auth.clone()) {
            Ok(Some(fresh)) => {
                *self.inner.write().await = Arc::new(fresh);
                tracing::info!(path = %path.display(), "providers config reloaded");
            }
            Ok(None) => {
                tracing::warn!(
                    path = %path.display(),
                    "providers config removed; keeping last-good router"
                );
                // Refresh cached mtime so we don't re-parse every request.
                let mut guard = self.inner.write().await;
                let cloned = ProviderRouter {
                    default: guard.default.clone(),
                    by_model: guard.by_model.clone(),
                    models: guard.models.clone(),
                    all_disabled: guard.all_disabled,
                    source_path: guard.source_path.clone(),
                    source_mtime: mtime,
                };
                *guard = Arc::new(cloned);
            }
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "providers config reload failed; keeping last-good router"
                );
                // Refresh cached mtime to avoid re-parsing each request.
                let mut guard = self.inner.write().await;
                let cloned = ProviderRouter {
                    default: guard.default.clone(),
                    by_model: guard.by_model.clone(),
                    models: guard.models.clone(),
                    all_disabled: guard.all_disabled,
                    source_path: guard.source_path.clone(),
                    source_mtime: mtime,
                };
                *guard = Arc::new(cloned);
            }
        }
    }
}

impl ProviderRouter {
    /// Resolve the provider serving `model`, falling back to the default.
    pub fn resolve(&self, model: &str) -> &Arc<dyn Provider> {
        self.by_model
            .get(model)
            .unwrap_or(&self.default)
    }

    /// Build the router from the declarative config, falling back to the
    /// env-var single-provider setup when no file exists.
    pub fn from_env(config: &cmdcode_core::config::ProxyConfig, auth: Arc<AuthManager>) -> Self {
        let loaded = cmdcode_core::provider_config::ProvidersConfig::load()
            .ok()
            .flatten();

        let Some(cfg) = loaded else {
            // Back-compat: single provider from COMMAND_CODE_* env vars.
            let default: Arc<dyn Provider> = match config.provider.as_str() {
                "openai" => Arc::new(openai::OpenAiProvider {
                    base_url: config.upstream_url.clone(),
                    api_key: config.provider_api_key.clone(),
                }),
                _ => Arc::new(commandcode::CommandCodeProvider {
                    auth,
                    base_url: config.upstream_url.clone(),
                    learning: true,
                }),
            };
            return Self {
                default,
                by_model: std::collections::HashMap::new(),
                models: Vec::new(),
                all_disabled: false,
                source_path: None,
                source_mtime: None,
            };
        };

        let mut by_model = std::collections::HashMap::new();
        let mut models = Vec::new();
        let mut default: Option<Arc<dyn Provider>> = None;
        let source_path = cmdcode_core::provider_config::ProvidersConfig::default_path();
        let source_mtime = source_path
            .as_ref()
            .and_then(|p| std::fs::metadata(p).ok())
            .and_then(|m| m.modified().ok());

        let mut enabled_seen = false;
        for (key, entry) in cfg.entries() {
            if !entry.is_enabled() {
                tracing::info!(provider = %key, "skipping disabled provider");
                continue;
            }
            enabled_seen = true;
            let provider: Arc<dyn Provider> = match entry.kind() {
                cmdcode_core::provider_config::AdapterKind::OpenAi => Arc::new(openai::OpenAiProvider {
                    base_url: entry.base_url().unwrap_or_else(|| "http://localhost".into()),
                    api_key: entry.api_key(),
                }),
                cmdcode_core::provider_config::AdapterKind::Anthropic => {
                    Arc::new(anthropic::AnthropicProvider {
                        base_url: entry
                            .base_url()
                            .unwrap_or_else(|| "https://api.anthropic.com".into()),
                        api_key: entry.api_key(),
                    })
                }
                cmdcode_core::provider_config::AdapterKind::Gemini => {
                    Arc::new(gemini::GeminiProvider {
                        base_url: entry
                            .base_url()
                            .unwrap_or_else(|| gemini::DEFAULT_BASE.into()),
                        api_key: entry.api_key(),
                    })
                }
                cmdcode_core::provider_config::AdapterKind::CommandCode => Arc::new(
                    commandcode::CommandCodeProvider {
                        auth: auth.clone(),
                        base_url: entry
                            .base_url()
                            .unwrap_or_else(|| config.upstream_url.clone()),
                        learning: entry.learning,
                    },
                ),
            };
            if default.is_none() {
                default = Some(provider.clone());
            }
            let display_name =
                entry.name.clone().unwrap_or_else(|| key.to_string());
            for id in entry.models.keys() {
                by_model.insert(id.clone(), provider.clone());
                models.push(serde_json::json!({
                    "id": id,
                    "object": "model",
                    "created": 0,
                    "owned_by": display_name,
                }));
            }
        }

        // Every entry disabled: keep routing alive so requests get a clear
        // 503 instead of falling through to an unexpected backend.
        let default = default.unwrap_or_else(|| Arc::new(NoEnabledProvider));
        let _ = enabled_seen;
        Self {
            default,
            by_model,
            models,
            all_disabled: !enabled_seen && !cfg.providers.is_empty(),
            source_path,
            source_mtime,
        }
    }

    /// Build from a declarative providers file, returning Err when the file
    /// exists but cannot be parsed (used by hot reload to retain the
    /// last-good router instead of silently degrading). `Ok(None)` when no
    /// file exists.
    pub fn try_build_declared(
        config: &cmdcode_core::config::ProxyConfig,
        auth: Arc<AuthManager>,
    ) -> Result<Option<Self>, String> {
        match cmdcode_core::provider_config::ProvidersConfig::load() {
            Err(e) => Err(e),
            Ok(None) => Ok(None),
            Ok(Some(cfg)) => {
                let mut by_model = std::collections::HashMap::new();
                let mut models = Vec::new();
                let mut default: Option<Arc<dyn Provider>> = None;
                let source_path = cmdcode_core::provider_config::ProvidersConfig::default_path();
                let source_mtime = source_path
                    .as_ref()
                    .and_then(|p| std::fs::metadata(p).ok())
                    .and_then(|m| m.modified().ok());

                for (key, entry) in cfg.entries() {
                    if !entry.is_enabled() {
                        tracing::info!(provider = %key, "skipping disabled provider");
                        continue;
                    }
                    let provider: Arc<dyn Provider> = match entry.kind() {
                        cmdcode_core::provider_config::AdapterKind::OpenAi => {
                            Arc::new(openai::OpenAiProvider {
                                base_url: entry
                                    .base_url()
                                    .unwrap_or_else(|| "http://localhost".into()),
                                api_key: entry.api_key(),
                            })
                        }
                        cmdcode_core::provider_config::AdapterKind::Anthropic => {
                            Arc::new(anthropic::AnthropicProvider {
                                base_url: entry
                                    .base_url()
                                    .unwrap_or_else(|| "https://api.anthropic.com".into()),
                                api_key: entry.api_key(),
                            })
                        }
                        cmdcode_core::provider_config::AdapterKind::Gemini => {
                            Arc::new(gemini::GeminiProvider {
                                base_url: entry
                                    .base_url()
                                    .unwrap_or_else(|| "https://generativelanguage.googleapis.com/v1beta".into()),
                                api_key: entry.api_key(),
                            })
                        }
                        cmdcode_core::provider_config::AdapterKind::CommandCode => {
                            Arc::new(commandcode::CommandCodeProvider {
                                auth: auth.clone(),
                                base_url: entry
                                    .base_url()
                                    .unwrap_or_else(|| config.upstream_url.clone()),
                                learning: entry.learning,
                            })
                        }
                    };
                    if default.is_none() {
                        default = Some(provider.clone());
                    }
                    let display_name = entry.name.clone().unwrap_or_else(|| key.to_string());
                    for id in entry.models.keys() {
                        by_model.insert(id.clone(), provider.clone());
                        models.push(serde_json::json!({
                            "id": id,
                            "object": "model",
                            "created": 0,
                            "owned_by": display_name,
                        }));
                    }
                }

                let any_enabled = cfg.entries().any(|(_, e)| e.is_enabled());
                let non_empty = !cfg.providers.is_empty();
                let default = default.unwrap_or_else(|| Arc::new(NoEnabledProvider));
                Ok(Some(Self {
                    default,
                    by_model,
                    models,
                    all_disabled: non_empty && !any_enabled,
                    source_path,
                    source_mtime,
                }))
            }
        }
    }
}
