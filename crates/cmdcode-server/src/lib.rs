//! HTTP proxy server that translates OpenAI-compatible requests to the Command
//! Code upstream API.

/// Request handler implementing the pingora `ProxyHttp` trait.
pub mod handler;
/// Rotating log file writer for tracing output.
pub mod logging;
/// Prometheus-style metrics counters and rendering.
pub mod metrics;
/// Upstream HTTP client with retry and SSE streaming.
pub mod upstream;

use cmdcode_core::auth::AuthManager;
use cmdcode_core::config::ProxyConfig;
use cmdcode_core::rate_limiter::{RateLimiter, RateLimitBackend, RateLimitConfig};
use pingora_core::server::Server;
use std::sync::Arc;

use crate::metrics::Metrics;
use crate::upstream::UpstreamClient;

/// Top-level proxy service that owns configuration and starts the server.
pub struct ProxyService {
    /// Proxy configuration.
    pub config: Arc<ProxyConfig>,
    /// Authentication credential manager.
    pub auth: Arc<AuthManager>,
}

impl ProxyService {
    /// Create a new proxy service from the given config and auth manager.
    pub fn new(config: ProxyConfig, auth: AuthManager) -> Self {
        Self {
            config: Arc::new(config),
            auth: Arc::new(auth),
        }
    }

    /// Start the proxy server and run forever.
    pub fn run(self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Require auth token when binding to non-localhost.
        let host = self.config.listen_addr.split(':').next().unwrap_or("");
        if host != "127.0.0.1" && host != "localhost" && self.config.incoming_token.is_none() {
            tracing::error!(
                listen_addr = %self.config.listen_addr,
                "binding to non-localhost requires COMMAND_CODE_PROXY_INCOMING_TOKEN"
            );
            return Err("auth token required for non-localhost binding".into());
        }

        // Warn when upstream URL is HTTP (credentials sent in plaintext).
        if self.config.upstream_url.starts_with("http://") {
            tracing::warn!(
                upstream_url = %self.config.upstream_url,
                "upstream uses HTTP - credentials sent in plaintext, use HTTPS for production"
            );
        }

        let mut server = Server::new(None)?;
        server.bootstrap();

        let listen_addr = self.config.listen_addr.clone();
        let tls_cert = self
            .config
            .tls_cert
            .as_ref()
            .map(|p| p.display().to_string());
        let tls_key = self
            .config
            .tls_key
            .as_ref()
            .map(|p| p.display().to_string());
        let metrics = Arc::new(Metrics::new());
        let upstream_client = Arc::new(UpstreamClient::new(
            self.config.clone(),
            self.auth.clone(),
            metrics.clone(),
        ));

        let rate_limit_backend = match self.config.rate_limit_backend {
            RateLimitBackend::Redis => RateLimitBackend::Redis,
            RateLimitBackend::Local => RateLimitBackend::Local,
        };

        let rate_limiter = Arc::new(RateLimiter::new(RateLimitConfig {
            max_requests: self.config.rate_limit_max_requests,
            window_secs: self.config.rate_limit_window_secs,
            backend: rate_limit_backend,
            redis_url: self.config.rate_limit_redis_url.clone(),
        }));

        tracing::info!(
            rate_limit_max = self.config.rate_limit_max_requests,
            rate_limit_window = self.config.rate_limit_window_secs,
            rate_limit_backend = %self.config.rate_limit_backend,
            "rate limiting configured"
        );

        let ctx = handler::CommandCodeProxy {
            config: self.config,
            auth: self.auth,
            upstream_client,
            metrics,
            rate_limiter,
        };

        let mut my_proxy = handler::create_http_proxy_service(&server.configuration, ctx);

        match (tls_cert, tls_key) {
            (Some(cert), Some(key)) => {
                my_proxy.add_tls(&listen_addr, &cert, &key)?;
            }
            (None, None) => {
                my_proxy.add_tcp(&listen_addr);
            }
            _ => {
                return Err("both COMMAND_CODE_PROXY_TLS_CERT and COMMAND_CODE_PROXY_TLS_KEY must be set together".into());
            }
        }

        server.add_service(my_proxy);

        server.run_forever();
    }
}

#[cfg(test)]
fn extract_host(url: &str) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let without_scheme = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let host = without_scheme
        .split('/')
        .next()
        .unwrap_or(without_scheme)
        .split(':')
        .next()
        .unwrap_or(without_scheme)
        .to_string();
    if host.is_empty() {
        return Err("empty host".into());
    }
    Ok(host)
}

#[cfg(test)]
fn extract_port(url: &str) -> u16 {
    if url.starts_with("https") {
        url.split(':')
            .nth(2)
            .and_then(|s| s.split('/').next())
            .and_then(|s| s.parse().ok())
            .unwrap_or(443)
    } else {
        url.split(':')
            .nth(2)
            .and_then(|s| s.split('/').next())
            .and_then(|s| s.parse().ok())
            .unwrap_or(80)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_extract_host() {
        assert_eq!(
            extract_host("https://api.commandcode.ai").unwrap(),
            "api.commandcode.ai"
        );
        assert_eq!(extract_host("http://localhost:9090").unwrap(), "localhost");
        assert_eq!(
            extract_host("https://api.commandcode.ai/alpha/generate").unwrap(),
            "api.commandcode.ai"
        );
    }

    #[test]
    fn test_extract_port() {
        assert_eq!(extract_port("https://api.commandcode.ai"), 443);
        assert_eq!(extract_port("http://localhost:9090"), 9090);
        assert_eq!(extract_port("http://localhost"), 80);
        assert_eq!(extract_port("https://example.com:8443"), 8443);
    }

    #[test]
    fn test_proxy_service_creation() {
        let config = ProxyConfig {
            listen_addr: "127.0.0.1:18080".into(),
            upstream_url: "https://api.commandcode.ai".into(),
            default_model: "xiaomi/mimo-v2.5".into(),
            upstream_timeout_secs: 600,
            max_retries: 2,
            max_concurrent: 0,
            cors_origin: None,
            model_allowlist: None,
            auth_dir: PathBuf::from("/tmp/test/.commandcode"),
            auth_cache_ttl_secs: 30,
            log_level: "info".into(),
            max_body_size: 10 * 1024 * 1024,
            stream_idle_timeout_secs: 180,
            log_file: None,
            log_max_bytes: 50 * 1024 * 1024,
            log_keep: 5,
            tls_cert: None,
            tls_key: None,
            incoming_token: None,
            rate_limit_max_requests: 100,
            rate_limit_window_secs: 60,
            rate_limit_backend: cmdcode_core::types::RateLimitBackend::Local,
            rate_limit_redis_url: None,
        };
        let auth = AuthManager::new(config.auth_dir.clone(), 30);
        let _service = ProxyService::new(config, auth);
    }
}
