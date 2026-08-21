use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Supported harness types that can be configured to use cmdcode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HarnessType {
    /// OpenCode - AI coding assistant
    OpenCode,
    /// Codex CLI - OpenAI's coding agent
    Codex,
    /// Hermes - AI assistant with tool-calling capabilities
    Hermes,
    /// LiteLLM - LLM proxy and router
    LiteLLM,
    /// Ollama - Local LLM runtime
    Ollama,
    /// vLLM - High-throughput LLM inference engine
    Vllm,
    /// Open WebUI - Web interface for LLMs
    OpenWebUI,
    /// Custom harness type
    Custom(String),
}

impl HarnessType {
    /// Get the display name of this harness type.
    pub fn name(&self) -> &str {
        match self {
            Self::OpenCode => "OpenCode",
            Self::Codex => "Codex CLI",
            Self::Hermes => "Hermes",
            Self::LiteLLM => "LiteLLM",
            Self::Ollama => "Ollama",
            Self::Vllm => "vLLM",
            Self::OpenWebUI => "Open WebUI",
            Self::Custom(name) => name,
        }
    }

    /// Parse a user-provided string into a HarnessType.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        // F-11: Sanitize input to prevent path traversal
        let sanitized = s
            .replace('\\', "")
            .replace('/', "")
            .replace("..", "")
            .replace('\0', "");

        if sanitized.is_empty() {
            return None;
        }

        match sanitized.to_lowercase().as_str() {
            "opencode" | "open-code" | "open_code" => Some(Self::OpenCode),
            "codex" => Some(Self::Codex),
            "hermes" => Some(Self::Hermes),
            "litellm" | "lite-llm" | "lite_llm" => Some(Self::LiteLLM),
            "ollama" => Some(Self::Ollama),
            "vllm" | "v-llm" => Some(Self::Vllm),
            "openwebui" | "open-webui" | "open_webui" => Some(Self::OpenWebUI),
            _ => Some(Self::Custom(sanitized)),
        }
    }

    /// Check if this harness type matches a filter string (case-insensitive).
    pub fn matches_filter(&self, filter: &str) -> bool {
        let f = filter.to_lowercase();
        match self {
            Self::OpenCode => matches!(f.as_str(), "opencode" | "open-code" | "open_code"),
            Self::Codex => f == "codex",
            Self::Hermes => f == "hermes",
            Self::LiteLLM => matches!(f.as_str(), "litellm" | "lite-llm" | "lite_llm"),
            Self::Ollama => f == "ollama",
            Self::Vllm => matches!(f.as_str(), "vllm" | "v-llm"),
            Self::OpenWebUI => matches!(f.as_str(), "openwebui" | "open-webui" | "open_webui"),
            Self::Custom(name) => name.eq_ignore_ascii_case(filter),
        }
    }

/// Get the config path for this harness type.
pub fn config_path(&self) -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    match self {
        Self::OpenCode => Some(home.join(".config/opencode/opencode.json")),
        Self::Codex => Some(home.join(".codex/config.toml")),
        Self::Hermes => Some(home.join(".hermes/config.yaml")),
        Self::LiteLLM => None, // No standard config path
        Self::Ollama => Some(home.join(".ollama/config.json")),
        Self::Vllm => None, // No standard config path
        Self::OpenWebUI => Some(home.join(".open-webui/config.json")),
        Self::Custom(_) => None,
    }
}
}

/// Validate a proxy URL to ensure it's safe.
pub fn validate_proxy_url(url: &str) -> bool {
    // Must start with http:// or https://
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return false;
    }
    // Must not contain path traversal
    if url.contains("..") {
        return false;
    }
    // Must not contain null bytes
    if url.contains('\0') {
        return false;
    }
    true
}

impl HarnessType {
    /// Check if this harness is installed.
    pub fn is_installed(&self) -> bool {
        match self {
            Self::OpenCode => {
                self.config_path()
                    .map(|p| p.exists())
                    .unwrap_or(false)
            }
            Self::Codex => {
                which_exists("codex")
                    || self.config_path()
                        .map(|p| p.exists())
                        .unwrap_or(false)
            }
            Self::Hermes => {
                which_exists("hermes")
                    || self.config_path()
                        .map(|p| p.exists())
                        .unwrap_or(false)
            }
            Self::LiteLLM => {
                which_exists("litellm")
                    || PathBuf::from("litellm_config.yaml").exists()
                    || PathBuf::from("litellm_config.json").exists()
            }
            Self::Ollama => which_exists("ollama"),
            Self::Vllm => which_exists("vllm"),
            Self::OpenWebUI => {
                which_exists("open-webui")
                    || docker_container_exists("open-webui")
            }
            Self::Custom(_) => false,
        }
    }

    /// Check if this harness is currently running.
    pub fn is_running(&self) -> bool {
        match self {
            Self::OpenCode => pgrep_running("opencode"),
            Self::Codex => pgrep_running("codex"),
            Self::Hermes => pgrep_running("hermes"),
            Self::LiteLLM => pgrep_running("litellm"),
            Self::Ollama => {
                std::process::Command::new("curl")
                    .args(["-s", "http://localhost:11434/api/tags"])
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false)
            }
            Self::Vllm => pgrep_running("vllm"),
            Self::OpenWebUI => docker_container_exists("open-webui"),
            Self::Custom(_) => false,
        }
    }

    /// Get the version of this harness.
    pub fn version(&self) -> Option<String> {
        let cmd = match self {
            Self::OpenCode => "opencode",
            Self::Codex => "codex",
            Self::Hermes => "hermes",
            Self::LiteLLM => "litellm",
            Self::Ollama => "ollama",
            Self::Vllm => "vllm",
            Self::OpenWebUI => "open-webui",
            Self::Custom(_) => return None,
        };
        std::process::Command::new(cmd)
            .args(["--version"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
    }
}

fn which_exists(cmd: &str) -> bool {
    std::process::Command::new("which")
        .arg(cmd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn pgrep_running(pattern: &str) -> bool {
    std::process::Command::new("pgrep")
        .args(["-f", pattern])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn docker_container_exists(name: &str) -> bool {
    std::process::Command::new("docker")
        .args(["ps", "-q", "-f", &format!("name={name}")])
        .output()
        .map(|o| {
            let stdout = String::from_utf8_lossy(&o.stdout);
            !stdout.trim().is_empty()
        })
        .unwrap_or(false)
}

/// Detect all installed harnesses on the system.
pub fn detect_harnesses() -> Vec<HarnessType> {
    let all_types = [
        HarnessType::OpenCode,
        HarnessType::Codex,
        HarnessType::Hermes,
        HarnessType::LiteLLM,
        HarnessType::Ollama,
        HarnessType::Vllm,
        HarnessType::OpenWebUI,
    ];

    all_types
        .into_iter()
        .filter(|h| h.is_installed())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_harness_type_name() {
        assert_eq!(HarnessType::OpenCode.name(), "OpenCode");
        assert_eq!(HarnessType::Codex.name(), "Codex CLI");
        assert_eq!(HarnessType::Hermes.name(), "Hermes");
        assert_eq!(HarnessType::LiteLLM.name(), "LiteLLM");
        assert_eq!(HarnessType::Ollama.name(), "Ollama");
        assert_eq!(HarnessType::Vllm.name(), "vLLM");
        assert_eq!(HarnessType::OpenWebUI.name(), "Open WebUI");
        assert_eq!(HarnessType::Custom("test".into()).name(), "test");
    }

    #[test]
    fn test_harness_type_from_str() {
        assert_eq!(HarnessType::from_str("opencode"), Some(HarnessType::OpenCode));
        assert_eq!(HarnessType::from_str("open-code"), Some(HarnessType::OpenCode));
        assert_eq!(HarnessType::from_str("OPENCODE"), Some(HarnessType::OpenCode));
        assert_eq!(HarnessType::from_str("codex"), Some(HarnessType::Codex));
        assert_eq!(HarnessType::from_str("hermes"), Some(HarnessType::Hermes));
        assert_eq!(HarnessType::from_str("litellm"), Some(HarnessType::LiteLLM));
        assert_eq!(HarnessType::from_str("ollama"), Some(HarnessType::Ollama));
        assert_eq!(HarnessType::from_str("vllm"), Some(HarnessType::Vllm));
        assert_eq!(HarnessType::from_str("openwebui"), Some(HarnessType::OpenWebUI));
        assert_eq!(
            HarnessType::from_str("custom-tool"),
            Some(HarnessType::Custom("custom-tool".into()))
        );
    }

    #[test]
    fn test_harness_type_matches_filter() {
        assert!(HarnessType::OpenCode.matches_filter("opencode"));
        assert!(HarnessType::OpenCode.matches_filter("open-code"));
        assert!(HarnessType::OpenCode.matches_filter("OPENCODE"));
        assert!(!HarnessType::OpenCode.matches_filter("codex"));

        assert!(HarnessType::Codex.matches_filter("codex"));
        assert!(!HarnessType::Codex.matches_filter("opencode"));

        assert!(HarnessType::Hermes.matches_filter("hermes"));
        assert!(!HarnessType::Hermes.matches_filter("opencode"));

        assert!(HarnessType::LiteLLM.matches_filter("litellm"));
        assert!(HarnessType::LiteLLM.matches_filter("lite-llm"));

        assert!(HarnessType::Ollama.matches_filter("ollama"));
        assert!(!HarnessType::Ollama.matches_filter("opencode"));

        assert!(HarnessType::Vllm.matches_filter("vllm"));
        assert!(HarnessType::Vllm.matches_filter("v-llm"));

        assert!(HarnessType::OpenWebUI.matches_filter("openwebui"));
        assert!(HarnessType::OpenWebUI.matches_filter("open-webui"));

        assert!(HarnessType::Custom("my-tool".into()).matches_filter("my-tool"));
        assert!(HarnessType::Custom("My-Tool".into()).matches_filter("my-tool"));
        assert!(!HarnessType::Custom("my-tool".into()).matches_filter("other"));
    }

    #[test]
    fn test_harness_type_config_path() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(
            HarnessType::OpenCode.config_path(),
            Some(home.join(".config/opencode/opencode.json"))
        );
        assert_eq!(
            HarnessType::Codex.config_path(),
            Some(home.join(".codex/config.toml"))
        );
        assert_eq!(
            HarnessType::Hermes.config_path(),
            Some(home.join(".hermes/config.yaml"))
        );
        assert_eq!(HarnessType::LiteLLM.config_path(), None);
        assert_eq!(
            HarnessType::Ollama.config_path(),
            Some(home.join(".ollama/config.json"))
        );
        assert_eq!(HarnessType::Vllm.config_path(), None);
        assert_eq!(
            HarnessType::OpenWebUI.config_path(),
            Some(home.join(".open-webui/config.json"))
        );
    }

    #[test]
    fn test_validate_proxy_url() {
        assert!(validate_proxy_url("http://localhost:8080"));
        assert!(validate_proxy_url("https://example.com"));
        assert!(!validate_proxy_url("ftp://example.com"));
        assert!(!validate_proxy_url("javascript:alert(1)"));
        assert!(!validate_proxy_url("http://example.com/../../../etc/passwd"));
        assert!(!validate_proxy_url("http://example.com\0/path"));
        assert!(!validate_proxy_url(""));
    }

    #[test]
    fn test_is_installed() {
        // Just verify the function doesn't panic on any harness type
        let _ = HarnessType::OpenCode.is_installed();
        let _ = HarnessType::Codex.is_installed();
        let _ = HarnessType::Hermes.is_installed();
        let _ = HarnessType::LiteLLM.is_installed();
        let _ = HarnessType::Ollama.is_installed();
        let _ = HarnessType::Vllm.is_installed();
        let _ = HarnessType::OpenWebUI.is_installed();
        let _ = HarnessType::Custom("test".into()).is_installed();
    }

    #[test]
    fn test_is_running() {
        // Just verify the function doesn't panic on any harness type
        let _ = HarnessType::OpenCode.is_running();
        let _ = HarnessType::Codex.is_running();
        let _ = HarnessType::Hermes.is_running();
        let _ = HarnessType::LiteLLM.is_running();
        let _ = HarnessType::Ollama.is_running();
        let _ = HarnessType::Vllm.is_running();
        let _ = HarnessType::OpenWebUI.is_running();
        let _ = HarnessType::Custom("test".into()).is_running();
    }

    #[test]
    fn test_version() {
        // Just verify the function doesn't panic on any harness type
        let _ = HarnessType::OpenCode.version();
        let _ = HarnessType::Codex.version();
        let _ = HarnessType::Hermes.version();
        let _ = HarnessType::LiteLLM.version();
        let _ = HarnessType::Ollama.version();
        let _ = HarnessType::Vllm.version();
        let _ = HarnessType::OpenWebUI.version();
        assert_eq!(HarnessType::Custom("test".into()).version(), None);
    }

    #[test]
    fn test_detect_harnesses() {
        // Just verify the function doesn't panic
        let harnesses = detect_harnesses();
        // Should return a Vec (may be empty)
        let _ = harnesses.len();
    }
}
