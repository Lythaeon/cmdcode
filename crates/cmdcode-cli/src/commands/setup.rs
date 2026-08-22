use cmdcode_core::setup::{validate_proxy_url, HarnessType};
use std::path::{Path, PathBuf};

/// Sanitize a path for display by replacing the home directory with ~.
fn sanitize_path_for_display(path: &Path) -> String {
    let home = dirs::home_dir().unwrap_or_default();
    if let Ok(stripped) = path.strip_prefix(&home) {
        format!("~/{}", stripped.display())
    } else {
        path.display().to_string()
    }
}

/// Run setup for all detected harnesses.
pub fn run_all(dry_run: bool, force: bool) {
    run(None, dry_run, force);
}

/// Run setup for a specific harness by name.
pub fn run_harness(harness_name: &str, dry_run: bool, force: bool) {
    run(Some(harness_name), dry_run, force);
}

/// Run the setup command.
fn run(harness_filter: Option<&str>, dry_run: bool, force: bool) {
    tracing::info!("running setup");

    let harnesses = detect_harnesses();

    if harnesses.is_empty() {
        tracing::warn!("no harnesses detected on this system");
        tracing::info!(
            "supported harnesses: opencode, codex, hermes, litellm, ollama, vllm, open-webui"
        );
        tracing::info!("install a harness first, then run: cmdcode setup");
        return;
    }

    tracing::info!(count = harnesses.len(), "harnesses detected");

    let proxy_url =
        std::env::var("COMMAND_CODE_PROXY_URL").unwrap_or_else(|_| "http://127.0.0.1:18080".into());

    // Validate proxy URL
    if !validate_proxy_url(&proxy_url) {
        tracing::error!(url = %proxy_url, "invalid proxy URL");
        tracing::info!("URL must start with http:// or https:// and not contain path traversal");
        std::process::exit(1);
    }

    let api_key = std::env::var("COMMAND_CODE_PROXY_INCOMING_TOKEN").ok();

    let default_model =
        std::env::var("COMMAND_CODE_PROXY_DEFAULT").unwrap_or_else(|_| "xiaomi/mimo-v2.5".into());

    for detected in &harnesses {
        let matches_filter = harness_filter
            .map(|f| {
                let f_lower = f.to_lowercase();
                detected.harness_type.matches_filter(&f_lower)
            })
            .unwrap_or(true);

        if !matches_filter {
            continue;
        }

        tracing::info!(
            harness = %detected.harness_type.name(),
            installed = true,
            version = detected.version.as_deref().unwrap_or("unknown"),
            config = detected.config_path.as_ref().map(|p| sanitize_path_for_display(p)).unwrap_or_else(|| "(none)".into()),
            running = detected.is_running,
            "harness detected"
        );

        let config = HarnessConfig {
            harness_type: detected.harness_type.clone(),
            proxy_url: proxy_url.clone(),
            api_key: api_key.clone(),
            default_model: default_model.clone(),
            extra: None,
        };

        match &detected.harness_type {
            HarnessType::OpenCode => {
                if dry_run {
                    tracing::info!("dry-run: would write OpenCode configuration");
                    let json = config.to_opencode_json();
                    let pretty = serde_json::to_string_pretty(&json).unwrap_or_default();
                    tracing::debug!(config = %pretty, "OpenCode config preview");
                } else {
                    match setup_opencode(&config, force) {
                        Ok(path) => tracing::info!(
                            path = %sanitize_path_for_display(&path),
                            "OpenCode configured"
                        ),
                        Err(e) => tracing::error!(error = %e, "failed to configure OpenCode"),
                    }
                }
            }
            HarnessType::Codex => {
                if dry_run {
                    tracing::info!("dry-run: would write Codex configuration");
                    let text = config.to_codex_toml();
                    tracing::debug!(config = %text, "Codex config preview");
                } else {
                    match setup_codex(&config, force) {
                        Ok(path) => tracing::info!(
                            path = %sanitize_path_for_display(&path),
                            "Codex configured"
                        ),
                        Err(e) => tracing::error!(error = %e, "failed to configure Codex"),
                    }
                }
            }
            HarnessType::Hermes => {
                if dry_run {
                    tracing::info!("dry-run: would write Hermes configuration");
                    let text = config.to_hermes_yaml();
                    tracing::debug!(config = %text, "Hermes config preview");
                } else {
                    match setup_hermes(&config, force) {
                        Ok(path) => tracing::info!(
                            path = %sanitize_path_for_display(&path),
                            "Hermes configured"
                        ),
                        Err(e) => tracing::error!(error = %e, "failed to configure Hermes"),
                    }
                }
            }
            HarnessType::LiteLLM => {
                if dry_run {
                    tracing::info!("dry-run: would write LiteLLM configuration");
                    let json = config.to_litellm_config();
                    let pretty = serde_json::to_string_pretty(&json).unwrap_or_default();
                    tracing::debug!(config = %pretty, "LiteLLM config preview");
                } else {
                    match setup_litellm(&config, force) {
                        Ok(path) => tracing::info!(
                            path = %sanitize_path_for_display(&path),
                            "LiteLLM configured"
                        ),
                        Err(e) => tracing::error!(error = %e, "failed to configure LiteLLM"),
                    }
                }
            }
            HarnessType::Ollama => {
                if dry_run {
                    tracing::info!("dry-run: would write Ollama configuration");
                    let text = config.to_ollama_config();
                    tracing::debug!(config = %text, "Ollama config preview");
                } else {
                    match setup_ollama(&config, force) {
                        Ok(path) => tracing::info!(
                            path = %sanitize_path_for_display(&path),
                            "Ollama configured"
                        ),
                        Err(e) => tracing::error!(error = %e, "failed to configure Ollama"),
                    }
                }
            }
            HarnessType::Vllm => {
                if dry_run {
                    tracing::info!("dry-run: would write vLLM configuration");
                    let text = config.to_vllm_config();
                    tracing::debug!(config = %text, "vLLM config preview");
                } else {
                    match setup_vllm(&config, force) {
                        Ok(path) => tracing::info!(
                            path = %sanitize_path_for_display(&path),
                            "vLLM configured"
                        ),
                        Err(e) => tracing::error!(error = %e, "failed to configure vLLM"),
                    }
                }
            }
            HarnessType::OpenWebUI => {
                if dry_run {
                    tracing::info!("dry-run: would write Open WebUI configuration");
                    let json = config.to_openwebui_config();
                    let pretty = serde_json::to_string_pretty(&json).unwrap_or_default();
                    tracing::debug!(config = %pretty, "Open WebUI config preview");
                } else {
                    match setup_openwebui(&config, force) {
                        Ok(path) => tracing::info!(
                            path = %sanitize_path_for_display(&path),
                            "Open WebUI configured"
                        ),
                        Err(e) => tracing::error!(error = %e, "failed to configure Open WebUI"),
                    }
                }
            }
            HarnessType::Custom(name) => {
                tracing::warn!(harness = %name, "skipping custom harness (no built-in support)");
            }
        }
    }

    tracing::info!("setup complete, start the proxy with: cmdcode serve");
}

/// Detected harness installation.
#[derive(Debug, Clone)]
pub struct DetectedHarness {
    pub harness_type: HarnessType,
    pub config_path: Option<PathBuf>,
    pub is_running: bool,
    pub version: Option<String>,
}

/// Configuration to apply for a harness.
#[derive(Debug, Clone)]
pub struct HarnessConfig {
    #[allow(dead_code)]
    pub harness_type: HarnessType,
    pub proxy_url: String,
    pub api_key: Option<String>,
    pub default_model: String,
    #[allow(dead_code)]
    pub extra: Option<serde_json::Value>,
}

impl HarnessConfig {
    pub fn to_opencode_json(&self) -> serde_json::Value {
        serde_json::json!({
            "provider": {
                "command-code": {
                    "npm": "@ai-sdk/openai-compatible",
                    "name": "Command Code",
                    "options": {
                        "baseURL": format!("{}/v1", self.proxy_url)
                    },
                    "models": {
                        "xiaomi/mimo-v2.5": { "name": "MiMo V2.5", "reasoning": true },
                        "gpt-5.6-luna": { "name": "GPT-5.6 Luna", "reasoning": true },
                        "claude-sonnet-5": { "name": "Claude Sonnet 5", "reasoning": true },
                        "deepseek/deepseek-v4-pro": { "name": "DeepSeek V4 Pro", "reasoning": true }
                    }
                }
            },
            "model": self.default_model.clone()
        })
    }

    pub fn to_codex_toml(&self) -> String {
        let model = self.default_model.replace('/', "-");
        format!(
            r#"# cmdcode proxy configuration for Codex CLI
# Edit ~/.codex/config.toml to apply these settings

# Model configuration
model = "{model}"
model_reasoning_effort = "medium"

# Proxy base URL (set as environment variable or in config)
# export OPENAI_BASE_URL="{proxy_url}/v1"
# export OPENAI_API_KEY="not-needed"

# Supported models (via proxy):
#   - xiaomi/mimo-v2.5 (MiMo V2.5)
#   - gpt-5.6-luna (GPT-5.6 Luna)
#   - claude-sonnet-5 (Claude Sonnet 5)
#   - deepseek/deepseek-v4-pro (DeepSeek V4 Pro)
"#,
            proxy_url = self.proxy_url
        )
    }

    pub fn to_hermes_yaml(&self) -> String {
        format!(
            r#"# cmdcode proxy configuration for Hermes
# Edit ~/.hermes/config.yaml to apply these settings

model:
  default: "{default_model}"
  provider: openai
  base_url: "{proxy_url}/v1"
  api_key: "not-needed"

# Supported models (via proxy):
#   - xiaomi/mimo-v2.5 (MiMo V2.5)
#   - gpt-5.6-luna (GPT-5.6 Luna)
#   - claude-sonnet-5 (Claude Sonnet 5)
#   - deepseek/deepseek-v4-pro (DeepSeek V4 Pro)
"#,
            default_model = self.default_model,
            proxy_url = self.proxy_url
        )
    }

    pub fn to_litellm_config(&self) -> serde_json::Value {
        let api_key = self.api_key.as_deref().unwrap_or("not-needed");
        serde_json::json!({
            "model_list": [
                {
                    "model_name": "command-code",
                    "litellm_params": {
                        "model": "openai/xiaomi/mimo-v2.5",
                        "api_base": self.proxy_url,
                        "api_key": api_key
                    }
                },
                {
                    "model_name": "command-code-gpt",
                    "litellm_params": {
                        "model": "openai/gpt-5.6-luna",
                        "api_base": self.proxy_url,
                        "api_key": api_key
                    }
                },
                {
                    "model_name": "command-code-claude",
                    "litellm_params": {
                        "model": "openai/claude-sonnet-5",
                        "api_base": self.proxy_url,
                        "api_key": api_key
                    }
                }
            ],
            "litellm_settings": {
                "drop_params": true
            }
        })
    }

    pub fn to_ollama_config(&self) -> String {
        format!(
            r#"# cmdcode proxy configuration for Ollama
# Add this to your Ollama config or set environment variables:
#
# Option 1: Environment variables (recommended)
export OLLAMA_HOST="http://127.0.0.1:11434"
export OLLAMA_MODELS="{proxy_url}/v1"
#
# Option 2: Use OpenAI-compatible endpoint directly
# The proxy exposes /v1/chat/completions which is OpenAI-compatible.
# Point your OpenAI client to: {proxy_url}/v1
#
# Option 3: Create a Modelfile for specific models
# FROM {proxy_url}/v1
# PARAMETER model xiaomi/mimo-v2.5
#
# Supported models (via proxy):
#   - xiaomi/mimo-v2.5 (MiMo V2.5)
#   - gpt-5.6-luna (GPT-5.6 Luna)
#   - claude-sonnet-5 (Claude Sonnet 5)
#   - deepseek/deepseek-v4-pro (DeepSeek V4 Pro)
"#,
            proxy_url = self.proxy_url
        )
    }

    pub fn to_vllm_config(&self) -> String {
        let api_key = self.api_key.as_deref().unwrap_or("not-needed");
        format!(
            r#"# cmdcode proxy configuration for vLLM
# Use the proxy as an OpenAI-compatible backend:
#
# Option 1: OpenAI client pointing to proxy
#   base_url = "{proxy_url}/v1"
#   api_key = "{api_key}"
#
# Option 2: LiteLLM config (see LiteLLM setup)
# Option 3: Direct API calls
#   POST {proxy_url}/v1/chat/completions
#     Content-Type: application/json
#     Authorization: Bearer {api_key}
#     {{"model":"xiaomi/mimo-v2.5","messages":[{{"role":"user","content":"Hello"}}]}}
#
# Supported models (via proxy):
#   - xiaomi/mimo-v2.5 (MiMo V2.5)
#   - gpt-5.6-luna (GPT-5.6 Luna)
#   - claude-sonnet-5 (Claude Sonnet 5)
#   - deepseek/deepseek-v4-pro (DeepSeek V4 Pro)
"#,
            proxy_url = self.proxy_url,
            api_key = api_key
        )
    }

    pub fn to_openwebui_config(&self) -> serde_json::Value {
        let api_key = self.api_key.as_deref().unwrap_or("not-needed");
        serde_json::json!({
            "OPENAI_API_BASE_URL": format!("{}/v1", self.proxy_url),
            "OPENAI_API_KEY": api_key,
            "DEFAULT_MODELS": "command-code:xiaomi/mimo-v2.5"
        })
    }
}

/// Detect installed harnesses on the system.
fn detect_harnesses() -> Vec<DetectedHarness> {
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
        .map(|h| DetectedHarness {
            harness_type: h.clone(),
            config_path: h.config_path(),
            is_running: h.is_running(),
            version: h.version(),
        })
        .collect()
}

// === Setup functions ===

fn setup_opencode(config: &HarnessConfig, force: bool) -> Result<PathBuf, String> {
    let config_dir = dirs::home_dir()
        .ok_or("cannot determine home directory")?
        .join(".config")
        .join("opencode");
    let config_path = config_dir.join("opencode.json");

    if config_path.exists() && !force {
        return Err(format!(
            "config already exists: {}. Use --force to overwrite",
            config_path.display()
        ));
    }

    std::fs::create_dir_all(&config_dir)
        .map_err(|e| format!("failed to create config directory: {e}"))?;

    let json = config.to_opencode_json();
    let content = serde_json::to_string_pretty(&json)
        .map_err(|e| format!("failed to serialize config: {e}"))?;

    std::fs::write(&config_path, content).map_err(|e| format!("failed to write config: {e}"))?;

    Ok(config_path)
}

fn setup_codex(config: &HarnessConfig, force: bool) -> Result<PathBuf, String> {
    let config_dir = dirs::home_dir()
        .ok_or("cannot determine home directory")?
        .join(".codex");
    let config_path = config_dir.join("cmdcode-proxy.toml");

    if config_path.exists() && !force {
        return Err(format!(
            "config already exists: {}. Use --force to overwrite",
            config_path.display()
        ));
    }

    std::fs::create_dir_all(&config_dir)
        .map_err(|e| format!("failed to create config directory: {e}"))?;

    let content = config.to_codex_toml();

    std::fs::write(&config_path, content).map_err(|e| format!("failed to write config: {e}"))?;

    Ok(config_path)
}

fn setup_hermes(config: &HarnessConfig, force: bool) -> Result<PathBuf, String> {
    let config_dir = dirs::home_dir()
        .ok_or("cannot determine home directory")?
        .join(".hermes");
    let config_path = config_dir.join("cmdcode-proxy.yaml");

    if config_path.exists() && !force {
        return Err(format!(
            "config already exists: {}. Use --force to overwrite",
            config_path.display()
        ));
    }

    std::fs::create_dir_all(&config_dir)
        .map_err(|e| format!("failed to create config directory: {e}"))?;

    let content = config.to_hermes_yaml();

    std::fs::write(&config_path, content).map_err(|e| format!("failed to write config: {e}"))?;

    Ok(config_path)
}

fn setup_litellm(config: &HarnessConfig, force: bool) -> Result<PathBuf, String> {
    let config_path = std::env::current_dir()
        .map_err(|e| format!("failed to get current directory: {e}"))?
        .join("litellm_config.json");

    if config_path.exists() && !force {
        return Err(format!(
            "config already exists: {}. Use --force to overwrite",
            config_path.display()
        ));
    }

    let json = config.to_litellm_config();
    let content = serde_json::to_string_pretty(&json)
        .map_err(|e| format!("failed to serialize config: {e}"))?;

    std::fs::write(&config_path, content).map_err(|e| format!("failed to write config: {e}"))?;

    Ok(config_path)
}

fn setup_ollama(config: &HarnessConfig, force: bool) -> Result<PathBuf, String> {
    let config_dir = dirs::home_dir()
        .ok_or("cannot determine home directory")?
        .join(".ollama");
    let config_path = config_dir.join("cmdcode-proxy.env");

    if config_path.exists() && !force {
        return Err(format!(
            "config already exists: {}. Use --force to overwrite",
            config_path.display()
        ));
    }

    std::fs::create_dir_all(&config_dir)
        .map_err(|e| format!("failed to create config directory: {e}"))?;

    let content = config.to_ollama_config();

    std::fs::write(&config_path, content).map_err(|e| format!("failed to write config: {e}"))?;

    Ok(config_path)
}

fn setup_vllm(config: &HarnessConfig, force: bool) -> Result<PathBuf, String> {
    let config_dir =
        std::env::current_dir().map_err(|e| format!("failed to get current directory: {e}"))?;
    let config_path = config_dir.join("cmdcode-proxy.env");

    if config_path.exists() && !force {
        return Err(format!(
            "config already exists: {}. Use --force to overwrite",
            config_path.display()
        ));
    }

    let content = config.to_vllm_config();

    std::fs::write(&config_path, content).map_err(|e| format!("failed to write config: {e}"))?;

    Ok(config_path)
}

fn setup_openwebui(config: &HarnessConfig, force: bool) -> Result<PathBuf, String> {
    let config_dir = dirs::home_dir()
        .ok_or("cannot determine home directory")?
        .join(".open-webui");
    let config_path = config_dir.join("cmdcode-config.json");

    if config_path.exists() && !force {
        return Err(format!(
            "config already exists: {}. Use --force to overwrite",
            config_path.display()
        ));
    }

    std::fs::create_dir_all(&config_dir)
        .map_err(|e| format!("failed to create config directory: {e}"))?;

    let json = config.to_openwebui_config();
    let content = serde_json::to_string_pretty(&json)
        .map_err(|e| format!("failed to serialize config: {e}"))?;

    std::fs::write(&config_path, content).map_err(|e| format!("failed to write config: {e}"))?;

    Ok(config_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_harness_type_name() {
        assert_eq!(HarnessType::OpenCode.name(), "OpenCode");
        assert_eq!(HarnessType::LiteLLM.name(), "LiteLLM");
        assert_eq!(HarnessType::Ollama.name(), "Ollama");
        assert_eq!(HarnessType::Vllm.name(), "vLLM");
        assert_eq!(HarnessType::OpenWebUI.name(), "Open WebUI");
        assert_eq!(HarnessType::Custom("test".into()).name(), "test");
    }

    #[test]
    fn test_opencode_json_generation() {
        let config = HarnessConfig {
            harness_type: HarnessType::OpenCode,
            proxy_url: "http://127.0.0.1:18080".into(),
            api_key: None,
            default_model: "xiaomi/mimo-v2.5".into(),
            extra: None,
        };
        let json = config.to_opencode_json();
        assert!(json.get("provider").is_some());
        assert!(json
            .get("provider")
            .and_then(|p| p.get("command-code"))
            .is_some());
    }

    #[test]
    fn test_litellm_config_generation() {
        let config = HarnessConfig {
            harness_type: HarnessType::LiteLLM,
            proxy_url: "http://127.0.0.1:18080".into(),
            api_key: None,
            default_model: "xiaomi/mimo-v2.5".into(),
            extra: None,
        };
        let json = config.to_litellm_config();
        assert!(json.get("model_list").is_some());
        let models = json["model_list"].as_array().unwrap();
        assert_eq!(models.len(), 3);
    }

    #[test]
    fn test_ollama_config_generation() {
        let config = HarnessConfig {
            harness_type: HarnessType::Ollama,
            proxy_url: "http://127.0.0.1:18080".into(),
            api_key: None,
            default_model: "xiaomi/mimo-v2.5".into(),
            extra: None,
        };
        let text = config.to_ollama_config();
        assert!(text.contains("OLLAMA_HOST"));
        assert!(text.contains("127.0.0.1:18080"));
    }

    #[test]
    fn test_vllm_config_generation() {
        let config = HarnessConfig {
            harness_type: HarnessType::Vllm,
            proxy_url: "http://127.0.0.1:18080".into(),
            api_key: None,
            default_model: "xiaomi/mimo-v2.5".into(),
            extra: None,
        };
        let text = config.to_vllm_config();
        assert!(text.contains("base_url"));
        assert!(text.contains("127.0.0.1:18080"));
    }

    #[test]
    fn test_openwebui_config_generation() {
        let config = HarnessConfig {
            harness_type: HarnessType::OpenWebUI,
            proxy_url: "http://127.0.0.1:18080".into(),
            api_key: Some("test-key".into()),
            default_model: "xiaomi/mimo-v2.5".into(),
            extra: None,
        };
        let json = config.to_openwebui_config();
        assert_eq!(
            json["OPENAI_API_BASE_URL"].as_str().unwrap(),
            "http://127.0.0.1:18080/v1"
        );
        assert_eq!(json["OPENAI_API_KEY"].as_str().unwrap(), "test-key");
    }
}
