use crate::error::ConfigError;
use crate::types::RateLimitBackend;
use std::collections::HashSet;
use std::env;
use std::path::PathBuf;

/// Proxy configuration — all fields parsed from env or defaults.
#[derive(Debug, Clone)]
pub struct ProxyConfig {
    /// Address and port to listen on (e.g. `127.0.0.1:18080`).
    pub listen_addr: String,
    /// Upstream Command Code API base URL.
    pub upstream_url: String,
    /// Default model when the client does not specify one.
    pub default_model: String,
    /// Timeout in seconds for upstream requests.
    pub upstream_timeout_secs: u64,
    /// Maximum number of retries for transient upstream failures.
    pub max_retries: u32,
    /// Maximum concurrent upstream requests (0 = unlimited).
    pub max_concurrent: usize,
    /// CORS `Access-Control-Allow-Origin` value.
    pub cors_origin: Option<String>,
    /// Set of allowed model IDs (None = allow all).
    pub model_allowlist: Option<HashSet<String>>,
    /// Directory containing `auth.json` and `config.json`.
    pub auth_dir: PathBuf,
    /// How long to cache auth credentials before re-reading from disk.
    pub auth_cache_ttl_secs: u64,
    /// Tracing log level (e.g. `info`, `debug`).
    pub log_level: String,
    /// Maximum allowed request body size in bytes.
    pub max_body_size: usize,
    /// Idle timeout in seconds before an SSE stream is closed.
    pub stream_idle_timeout_secs: u64,
    /// Optional path to a log file for rotating log output.
    pub log_file: Option<PathBuf>,
    /// Maximum size in bytes before the log file rotates.
    pub log_max_bytes: u64,
    /// Number of rotated log files to keep.
    pub log_keep: usize,
    /// Optional path to a TLS certificate file.
    pub tls_cert: Option<PathBuf>,
    /// Optional path to a TLS private key file.
    pub tls_key: Option<PathBuf>,
    /// Optional bearer token clients must present to access the proxy.
    pub incoming_token: Option<String>,
    /// Rate limit: maximum requests per window per API key (0 = unlimited).
    pub rate_limit_max_requests: u64,
    /// Rate limit: window duration in seconds.
    pub rate_limit_window_secs: u64,
    /// Rate limit: backend type (local or redis).
    pub rate_limit_backend: RateLimitBackend,
    /// Rate limit: Redis URL (only used if backend is "redis").
    pub rate_limit_redis_url: Option<String>,
}

impl ProxyConfig {
    /// Build a `ProxyConfig` by reading `COMMAND_CODE_*` environment variables.
    pub fn from_env() -> Result<Self, ConfigError> {
        let listen_host =
            env::var("COMMAND_CODE_PROXY_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
        if listen_host.contains(':') || listen_host.contains(char::is_whitespace) {
            return Err(ConfigError::InvalidListenAddress(format!(
                "COMMAND_CODE_PROXY_HOST contains invalid characters: {listen_host:?}"
            )));
        }
        let listen_port =
            env::var("COMMAND_CODE_PROXY_PORT").unwrap_or_else(|_| "18080".to_string());
        let port: u16 = listen_port.parse().map_err(|e| {
            ConfigError::InvalidListenAddress(format!(
                "COMMAND_CODE_PROXY_PORT is not a valid port: {listen_port:?}: {e}"
            ))
        })?;
        let listen_addr = format!("{}:{port}", listen_host);

        let upstream_url = env::var("COMMAND_CODE_API_BASE")
            .unwrap_or_else(|_| "https://api.commandcode.ai".to_string());

        let default_model = env::var("COMMAND_CODE_PROXY_DEFAULT")
            .unwrap_or_else(|_| "xiaomi/mimo-v2.5".to_string());

        let upstream_timeout_secs = env::var("COMMAND_CODE_PROXY_TIMEOUT")
            .unwrap_or_else(|_| "600".to_string())
            .parse()
            .map_err(|e| ConfigError::InvalidTimeout(format!("COMMAND_CODE_PROXY_TIMEOUT: {e}")))?;

        let max_retries = env::var("COMMAND_CODE_PROXY_RETRIES")
            .unwrap_or_else(|_| "2".to_string())
            .parse()
            .map_err(|e| ConfigError::InvalidTimeout(format!("COMMAND_CODE_PROXY_RETRIES: {e}")))?;

        let max_concurrent = env::var("COMMAND_CODE_PROXY_MAX_REQS")
            .unwrap_or_else(|_| "0".to_string())
            .parse()
            .map_err(|e| {
                ConfigError::InvalidTimeout(format!("COMMAND_CODE_PROXY_MAX_REQS: {e}"))
            })?;

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

        let log_level =
            env::var("COMMAND_CODE_PROXY_LOG_LEVEL").unwrap_or_else(|_| "info".to_string());

        let max_body_size = env::var("COMMAND_CODE_PROXY_MAX_BODY_SIZE")
            .unwrap_or_else(|_| "10485760".to_string()) // 10MB default
            .parse()
            .unwrap_or(10 * 1024 * 1024);

        let stream_idle_timeout_secs = env::var("COMMAND_CODE_PROXY_STREAM_IDLE_TIMEOUT")
            .unwrap_or_else(|_| "180".to_string())
            .parse()
            .unwrap_or(180);

        let log_file = env::var("COMMAND_CODE_PROXY_LOG_FILE")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from);

        let log_max_bytes = env::var("COMMAND_CODE_PROXY_LOG_MAX_BYTES")
            .unwrap_or_else(|_| "52428800".to_string()) // 50MB default
            .parse()
            .unwrap_or(50 * 1024 * 1024);

        let log_keep = env::var("COMMAND_CODE_PROXY_LOG_KEEP")
            .unwrap_or_else(|_| "5".to_string())
            .parse()
            .unwrap_or(5)
            .max(1);

        let tls_cert = env::var("COMMAND_CODE_PROXY_TLS_CERT")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from);

        let tls_key = env::var("COMMAND_CODE_PROXY_TLS_KEY")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from);

        let incoming_token = env::var("COMMAND_CODE_PROXY_INCOMING_TOKEN")
            .ok()
            .filter(|s| !s.trim().is_empty());

        let rate_limit_max_requests = env::var("COMMAND_CODE_PROXY_RATE_LIMIT_MAX")
            .unwrap_or_else(|_| "100".to_string())
            .parse()
            .unwrap_or(100);

        let rate_limit_window_secs = env::var("COMMAND_CODE_PROXY_RATE_LIMIT_WINDOW")
            .unwrap_or_else(|_| "60".to_string())
            .parse()
            .unwrap_or(60);

        let rate_limit_backend = env::var("COMMAND_CODE_PROXY_RATE_LIMIT_BACKEND")
            .ok()
            .and_then(|s| RateLimitBackend::from_str_opt(&s))
            .unwrap_or(RateLimitBackend::Local);

        let rate_limit_redis_url = env::var("COMMAND_CODE_PROXY_RATE_LIMIT_REDIS_URL").ok();

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
            log_file,
            log_max_bytes,
            log_keep,
            tls_cert,
            tls_key,
            incoming_token,
            rate_limit_max_requests,
            rate_limit_window_secs,
            rate_limit_backend,
            rate_limit_redis_url,
        })
    }
}
