//! Declarative upstream-provider configuration (opencode-style).
//!
//! Loaded from `~/.cmdcode/providers.json` (override with
//! `CMDCODE_PROVIDERS_CONFIG`). Mirrors opencode's `provider` map:
//!
//! ```json
//! {
//!   "providers": {
//!     "command-code": {
//!       "type": "command-code",
//!       "name": "Command Code",
//!       "options": { "baseURL": "https://api.commandcode.ai" },
//!       "models": { "xiaomi/mimo-v2.5": { "name": "MiMo v2.5" } }
//!     },
//!     "openai": {
//!       "type": "openai",
//!       "options": {
//!         "baseURL": "https://api.openai.com/v1",
//!         "apiKey": "{env:OPENAI_API_KEY}"
//!       }
//!     }
//!   }
//! }
//! ```
//!
//! String values support `{env:VAR}` interpolation like opencode.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Adapter kind for a provider entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterKind {
    /// Command Code `/alpha/generate` protocol.
    CommandCode,
    /// Generic OpenAI-compatible chat completions.
    OpenAi,
    /// Native Anthropic Messages API (`/v1/messages` upstream).
    Anthropic,
    /// Native Google Gemini API (`:generateContent` upstream).
    Gemini,
}

impl AdapterKind {
    /// Parse an adapter kind from its config string form.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "command-code" | "commandcode" | "cmd" => Some(Self::CommandCode),
            "openai" | "openai-compatible" => Some(Self::OpenAi),
            "anthropic" | "claude" => Some(Self::Anthropic),
            "gemini" | "google" => Some(Self::Gemini),
            _ => None,
        }
    }
}

/// Connection options for one provider entry.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ProviderOptions {
    /// Upstream base URL (`baseURL`, opencode-style).
    #[serde(rename = "baseURL", alias = "base_url")]
    pub base_url: Option<String>,
    /// Bearer token (`apiKey`, opencode-style).
    #[serde(rename = "apiKey", alias = "api_key")]
    pub api_key: Option<String>,
}

/// One declared upstream provider.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ProviderEntry {
    /// Display name.
    #[serde(default)]
    pub name: Option<String>,
    /// Adapter kind; defaults to `openai`.
    #[serde(rename = "type", alias = "adapter", default)]
    pub adapter: Option<String>,
    /// Connection options.
    #[serde(default)]
    pub options: ProviderOptions,
    /// Models exposed by this provider: id -> metadata (name/owned_by/etc).
    #[serde(default)]
    pub models: BTreeMap<String, serde_json::Value>,
    /// Whether this entry should serve taste learning requests.
    #[serde(default)]
    pub learning: bool,
    /// Whether this entry is active. Disabled entries serve no traffic;
    /// toggle at runtime with `cmdcode connect enable/disable`.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

impl ProviderEntry {
    /// Whether the entry accepts traffic (absent field defaults to enabled).
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Resolved adapter kind (default: openai).
    pub fn kind(&self) -> AdapterKind {
        self.adapter
            .as_deref()
            .and_then(AdapterKind::parse)
            .unwrap_or(AdapterKind::OpenAi)
    }

    /// Effective base URL with `{env:...}` interpolation applied.
    pub fn base_url(&self) -> Option<String> {
        self.options.base_url.as_deref().map(interpolate_env)
    }

    /// Effective API key with `{env:...}` interpolation applied.
    pub fn api_key(&self) -> Option<String> {
        self.options.api_key.as_deref().map(interpolate_env)
    }
}

/// Root of `providers.json`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ProvidersConfig {
    /// Declared providers keyed by their config id.
    #[serde(default)]
    pub providers: BTreeMap<String, ProviderEntry>,
}

impl ProvidersConfig {
    /// Config file path: `$CMDCODE_PROVIDERS_CONFIG` or `~/.cmdcode/providers.json`.
    pub fn default_path() -> Option<PathBuf> {
        if let Ok(p) = std::env::var("CMDCODE_PROVIDERS_CONFIG") {
            return Some(PathBuf::from(p));
        }
        dirs::home_dir().map(|h| h.join(".cmdcode").join("providers.json"))
    }

    /// Load and parse the config. Returns `Ok(None)` when no file exists.
    pub fn load() -> Result<Option<Self>, String> {
        let Some(path) = Self::default_path() else {
            return Ok(None);
        };
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("read {}: {e}", path.display())),
        };
        // Tolerate an empty file.
        if content.trim().is_empty() {
            return Ok(None);
        }
        let cfg: Self =
            serde_json::from_str(&content).map_err(|e| format!("parse {}: {e}", path.display()))?;
        Ok(Some(cfg))
    }

    /// Iterate entries in declaration order (BTreeMap key order).
    pub fn entries(&self) -> impl Iterator<Item = (&String, &ProviderEntry)> {
        self.providers.iter()
    }
}

/// Interpolate `{env:VAR}` references from the process environment.
fn interpolate_env(value: &str) -> String {
    if !value.contains("{env:") {
        return value.to_string();
    }
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find("{env:") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 5..];
        match after.find('}') {
            Some(end) => {
                let var = &after[..end];
                out.push_str(std::env::var(var).unwrap_or_default().as_str());
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_full_config() {
        let json = r#"{
            "providers": {
                "command-code": {
                    "type": "command-code",
                    "name": "Command Code",
                    "options": {"baseURL": "https://api.commandcode.ai"},
                    "models": {"xiaomi/mimo-v2.5": {"name": "MiMo"}}
                },
                "openai": {
                    "type": "openai",
                    "options": {"baseURL": "https://api.openai.com/v1", "apiKey": "{env:TEST_KEY}"},
                    "models": {"gpt-5.6-luna": {}}
                },
                "local-ollama": {
                    "options": {"baseURL": "http://127.0.0.1:11434/v1"}
                }
            }
        }"#;
        let cfg: ProvidersConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.providers.len(), 3);

        let cc = &cfg.providers["command-code"];
        assert_eq!(cc.kind(), AdapterKind::CommandCode);
        assert_eq!(cc.base_url().as_deref(), Some("https://api.commandcode.ai"));
        assert!(cc.models.contains_key("xiaomi/mimo-v2.5"));

        let oa = &cfg.providers["openai"];
        assert_eq!(oa.kind(), AdapterKind::OpenAi);
        assert_eq!(oa.models.len(), 1);
    }

    #[test]
    fn test_env_interpolation() {
        std::env::set_var("PROVIDER_CFG_TEST_KEY", "sk-test-123");
        assert_eq!(
            interpolate_env("{env:PROVIDER_CFG_TEST_KEY}"),
            "sk-test-123"
        );
        assert_eq!(
            interpolate_env("prefix-{env:PROVIDER_CFG_TEST_KEY}-suffix"),
            "prefix-sk-test-123-suffix"
        );
        assert_eq!(interpolate_env("{env:PROVIDER_CFG_TEST_UNSET_VAR_XYZ}"), "");
        assert_eq!(interpolate_env("plain"), "plain");
        std::env::remove_var("PROVIDER_CFG_TEST_KEY");
    }

    #[test]
    fn test_adapter_kind_defaults() {
        let entry: ProviderEntry = serde_json::from_str(r#"{"options":{}}"#).unwrap();
        assert_eq!(entry.kind(), AdapterKind::OpenAi);
        let entry: ProviderEntry = serde_json::from_str(r#"{"type":"command-code"}"#).unwrap();
        assert_eq!(entry.kind(), AdapterKind::CommandCode);
        let entry: ProviderEntry = serde_json::from_str(r#"{"type":"anthropic"}"#).unwrap();
        assert_eq!(entry.kind(), AdapterKind::Anthropic);
        let entry: ProviderEntry = serde_json::from_str(r#"{"type":"gemini"}"#).unwrap();
        assert_eq!(entry.kind(), AdapterKind::Gemini);
    }

    #[test]
    fn test_load_missing_returns_none() {
        // Path override to something that does not exist.
        std::env::set_var(
            "CMDCODE_PROVIDERS_CONFIG",
            "/tmp/nonexistent-providers-cfg.json",
        );
        let result = ProvidersConfig::load().unwrap();
        assert!(result.is_none());
        std::env::remove_var("CMDCODE_PROVIDERS_CONFIG");
    }
}
