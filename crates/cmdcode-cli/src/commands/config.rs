use cmdcode_core::config::ProxyConfig;

pub fn run() {
    tracing::info!("showing configuration");

    match ProxyConfig::from_env() {
        Ok(config) => {
            tracing::info!(listen = %config.listen_addr, "listen address");
            tracing::info!(upstream = %config.upstream_url, "upstream URL");
            tracing::info!(default_model = %config.default_model, "default model");
            tracing::info!(timeout = config.upstream_timeout_secs, "upstream timeout");
            tracing::info!(max_retries = config.max_retries, "max retries");
            tracing::info!(max_concurrent = config.max_concurrent, "max concurrent");
            tracing::info!(
                cors_origin = config.cors_origin.as_deref().unwrap_or("(not set)"),
                "CORS origin"
            );
            tracing::info!(
                model_allowlist = config
                    .model_allowlist
                    .as_ref()
                    .map(|s| s.iter().cloned().collect::<Vec<_>>().join(", "))
                    .unwrap_or_else(|| "(all allowed)".into()),
                "model allowlist"
            );
            tracing::info!(auth_dir = %config.auth_dir.display(), "auth directory");
            tracing::info!(
                auth_cache_ttl = config.auth_cache_ttl_secs,
                "auth cache TTL"
            );
            tracing::info!(log_level = %config.log_level, "log level");
            tracing::info!(max_body_size = config.max_body_size, "max body size");
            tracing::info!(
                stream_idle_timeout = config.stream_idle_timeout_secs,
                "stream idle timeout"
            );
            tracing::info!(
                log_file = config
                    .log_file
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "(stdout)".into()),
                "log file"
            );
            tracing::info!(log_max_bytes = config.log_max_bytes, "log max bytes");
            tracing::info!(log_keep = config.log_keep, "log keep");
            tracing::info!(
                tls_cert = config
                    .tls_cert
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "(not set)".into()),
                "TLS certificate"
            );
            tracing::info!(
                tls_key = config
                    .tls_key
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "(not set)".into()),
                "TLS key"
            );
            tracing::info!(
                incoming_token = config.incoming_token.is_some(),
                "incoming token"
            );
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to parse configuration");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serialize all tests that touch env vars (process-global).
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_config_from_env_defaults() {
        let _guard = ENV_LOCK.lock().unwrap();
        // Ensure clean env
        std::env::remove_var("COMMAND_CODE_PROXY_PORT");
        std::env::remove_var("COMMAND_CODE_PROXY_HOST");
        std::env::remove_var("COMMAND_CODE_PROXY_TIMEOUT");
        std::env::remove_var("COMMAND_CODE_PROXY_MODELS");
        std::env::remove_var("COMMAND_CODE_PROXY_INCOMING_TOKEN");
        std::env::remove_var("COMMAND_CODE_PROXY_LOG_FILE");

        let config = ProxyConfig::from_env().unwrap();
        assert!(!config.listen_addr.is_empty());
        assert!(!config.upstream_url.is_empty());
        assert!(!config.default_model.is_empty());
        assert!(config.upstream_timeout_secs > 0);
    }

    #[test]
    fn test_config_invalid_port() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("COMMAND_CODE_PROXY_PORT", "not-a-port");
        let result = ProxyConfig::from_env();
        assert!(result.is_err());
        std::env::remove_var("COMMAND_CODE_PROXY_PORT");
    }

    #[test]
    fn test_config_invalid_host() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("COMMAND_CODE_PROXY_HOST", "bad:host");
        let result = ProxyConfig::from_env();
        assert!(result.is_err());
        std::env::remove_var("COMMAND_CODE_PROXY_HOST");
    }

    #[test]
    fn test_config_whitespace_host() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("COMMAND_CODE_PROXY_HOST", "has space");
        let result = ProxyConfig::from_env();
        assert!(result.is_err());
        std::env::remove_var("COMMAND_CODE_PROXY_HOST");
    }

    #[test]
    fn test_config_invalid_timeout() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("COMMAND_CODE_PROXY_TIMEOUT", "not-a-number");
        let result = ProxyConfig::from_env();
        assert!(result.is_err());
        std::env::remove_var("COMMAND_CODE_PROXY_TIMEOUT");
    }

    #[test]
    fn test_config_empty_models() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("COMMAND_CODE_PROXY_MODELS", "");
        let config = ProxyConfig::from_env().unwrap();
        assert!(config.model_allowlist.is_none());
        std::env::remove_var("COMMAND_CODE_PROXY_MODELS");
    }

    #[test]
    fn test_config_whitespace_models() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var("COMMAND_CODE_PROXY_HOST");
        std::env::set_var("COMMAND_CODE_PROXY_MODELS", "  ,  ,  ");
        let config = ProxyConfig::from_env().unwrap();
        assert!(
            config.model_allowlist.is_none() || config.model_allowlist.as_ref().unwrap().is_empty()
        );
        std::env::remove_var("COMMAND_CODE_PROXY_MODELS");
    }

    #[test]
    fn test_config_tls_both_required() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var("COMMAND_CODE_PROXY_HOST");
        let config = ProxyConfig::from_env().unwrap();
        let _ = config.tls_cert;
        let _ = config.tls_key;
    }

    #[test]
    fn test_config_empty_incoming_token() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("COMMAND_CODE_PROXY_INCOMING_TOKEN", "  ");
        let config = ProxyConfig::from_env().unwrap();
        assert!(config.incoming_token.is_none());
        std::env::remove_var("COMMAND_CODE_PROXY_INCOMING_TOKEN");
    }

    #[test]
    fn test_config_empty_log_file() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("COMMAND_CODE_PROXY_LOG_FILE", "  ");
        let config = ProxyConfig::from_env().unwrap();
        assert!(config.log_file.is_none());
        std::env::remove_var("COMMAND_CODE_PROXY_LOG_FILE");
    }
}
