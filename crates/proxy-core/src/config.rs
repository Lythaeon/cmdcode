use crate::error::ConfigError;
use std::collections::HashSet;
use std::env;
use std::path::PathBuf;

/// Proxy configuration — all fields parsed from env or defaults.
#[derive(Debug, Clone)]
pub struct ProxyConfig {
    pub listen_addr: String,
    pub upstream_url: String,
    pub default_model: String,
    pub upstream_timeout_secs: u64,
    pub max_retries: u32,
    pub max_concurrent: usize,
    pub cors_origin: Option<String>,
    pub model_allowlist: Option<HashSet<String>>,
    pub auth_dir: PathBuf,
    pub auth_cache_ttl_secs: u64,
    pub log_level: String,
    pub max_body_size: usize,
    pub stream_idle_timeout_secs: u64,
}

impl ProxyConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        let listen_host = env::var("COMMAND_CODE_PROXY_HOST")
            .unwrap_or_else(|_| "127.0.0.1".to_string());
        let listen_port = env::var("COMMAND_CODE_PROXY_PORT")
            .unwrap_or_else(|_| "18080".to_string());
        let listen_addr = format!("{}:{}", listen_host, listen_port);

        let upstream_url = env::var("COMMAND_CODE_API_BASE")
            .unwrap_or_else(|_| "https://api.commandcode.ai".to_string());

        let default_model = env::var("COMMAND_CODE_PROXY_DEFAULT")
            .unwrap_or_else(|_| "xiaomi/mimo-v2.5".to_string());

        let upstream_timeout_secs = env::var("COMMAND_CODE_PROXY_TIMEOUT")
            .unwrap_or_else(|_| "600".to_string())
            .parse()
            .map_err(|_| ConfigError::InvalidTimeout("COMMAND_CODE_PROXY_TIMEOUT".into()))?;

        let max_retries = env::var("COMMAND_CODE_PROXY_RETRIES")
            .unwrap_or_else(|_| "2".to_string())
            .parse()
            .map_err(|_| ConfigError::InvalidTimeout("COMMAND_CODE_PROXY_RETRIES".into()))?;

        let max_concurrent = env::var("COMMAND_CODE_PROXY_MAX_REQS")
            .unwrap_or_else(|_| "0".to_string())
            .parse()
            .map_err(|_| ConfigError::InvalidTimeout("COMMAND_CODE_PROXY_MAX_REQS".into()))?;

        let cors_origin = env::var("COMMAND_CODE_PROXY_CORS").ok();

        let model_allowlist = env::var("COMMAND_CODE_PROXY_MODELS")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(|s| {
                s.split(',')
                    .map(|m| m.trim().to_string())
                    .filter(|m| !m.is_empty())
                    .collect::<HashSet<_>>()
            });

        let auth_dir = env::var("COMMAND_CODE_AUTH_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                dirs::home_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join(".commandcode")
            });

        let auth_cache_ttl_secs = env::var("COMMAND_CODE_AUTH_CACHE_TTL")
            .unwrap_or_else(|_| "30".to_string())
            .parse()
            .unwrap_or(30);

        let log_level = env::var("COMMAND_CODE_PROXY_LOG_LEVEL")
            .unwrap_or_else(|_| "info".to_string());

        let max_body_size = env::var("COMMAND_CODE_PROXY_MAX_BODY_SIZE")
            .unwrap_or_else(|_| "10485760".to_string())  // 10MB default
            .parse()
            .unwrap_or(10 * 1024 * 1024);

        let stream_idle_timeout_secs = env::var("COMMAND_CODE_PROXY_STREAM_IDLE_TIMEOUT")
            .unwrap_or_else(|_| "180".to_string())
            .parse()
            .unwrap_or(180);

        Ok(Self {
            listen_addr,
            upstream_url,
            default_model,
            upstream_timeout_secs,
            max_retries,
            max_concurrent,
            cors_origin,
            model_allowlist,
            auth_dir,
            auth_cache_ttl_secs,
            log_level,
            max_body_size,
            stream_idle_timeout_secs,
        })
    }
}
