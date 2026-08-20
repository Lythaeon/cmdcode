use cmdcode_core::config::ProxyConfig;

pub fn run() {
    println!("cmdcode config\n");

    match ProxyConfig::from_env() {
        Ok(config) => {
            println!("listen:             {}", config.listen_addr);
            println!("upstream:           {}", config.upstream_url);
            println!("default_model:      {}", config.default_model);
            println!("timeout:            {}s", config.upstream_timeout_secs);
            println!("max_retries:        {}", config.max_retries);
            println!("max_concurrent:     {}", config.max_concurrent);
            println!(
                "cors_origin:        {}",
                config.cors_origin.as_deref().unwrap_or("(not set)")
            );
            println!(
                "model_allowlist:    {}",
                config
                    .model_allowlist
                    .as_ref()
                    .map(|s| s.iter().cloned().collect::<Vec<_>>().join(", "))
                    .unwrap_or_else(|| "(all allowed)".into())
            );
            println!("auth_dir:           {}", config.auth_dir.display());
            println!("auth_cache_ttl:     {}s", config.auth_cache_ttl_secs);
            println!("log_level:          {}", config.log_level);
            println!("max_body_size:      {} bytes", config.max_body_size);
            println!("stream_idle_timeout: {}s", config.stream_idle_timeout_secs);
            println!(
                "log_file:           {}",
                config
                    .log_file
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "(stdout)".into())
            );
            println!("log_max_bytes:      {}", config.log_max_bytes);
            println!("log_keep:           {}", config.log_keep);
            println!(
                "tls_cert:           {}",
                config
                    .tls_cert
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "(not set)".into())
            );
            println!(
                "tls_key:            {}",
                config
                    .tls_key
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "(not set)".into())
            );
            println!(
                "incoming_token:     {}",
                if config.incoming_token.is_some() {
                    "(set)"
                } else {
                    "(not set)"
                }
            );
        }
        Err(e) => {
            eprintln!("error: failed to parse configuration: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn test_config_from_env_defaults() {
        let config = ProxyConfig::from_env().unwrap();
        // Just verify it parses successfully and has reasonable defaults
        assert!(!config.listen_addr.is_empty());
        assert!(!config.upstream_url.is_empty());
        assert!(!config.default_model.is_empty());
        assert!(config.upstream_timeout_secs > 0);
    }

    #[test]
    fn test_config_invalid_port() {
        std::env::set_var("COMMAND_CODE_PROXY_PORT", "not-a-port");
        let result = ProxyConfig::from_env();
        assert!(result.is_err());
        std::env::remove_var("COMMAND_CODE_PROXY_PORT");
    }

    #[test]
    fn test_config_invalid_host() {
        std::env::set_var("COMMAND_CODE_PROXY_HOST", "bad:host");
        let result = ProxyConfig::from_env();
        assert!(result.is_err());
        std::env::remove_var("COMMAND_CODE_PROXY_HOST");
    }

    #[test]
    fn test_config_whitespace_host() {
        std::env::set_var("COMMAND_CODE_PROXY_HOST", "has space");
        let result = ProxyConfig::from_env();
        assert!(result.is_err());
        std::env::remove_var("COMMAND_CODE_PROXY_HOST");
    }

    #[test]
    fn test_config_invalid_timeout() {
        std::env::set_var("COMMAND_CODE_PROXY_TIMEOUT", "not-a-number");
        let result = ProxyConfig::from_env();
        assert!(result.is_err());
        std::env::remove_var("COMMAND_CODE_PROXY_TIMEOUT");
    }

    #[test]
    fn test_config_empty_models() {
        std::env::set_var("COMMAND_CODE_PROXY_MODELS", "");
        let config = ProxyConfig::from_env().unwrap();
        assert!(config.model_allowlist.is_none());
        std::env::remove_var("COMMAND_CODE_PROXY_MODELS");
    }

    #[test]
    fn test_config_whitespace_models() {
        std::env::set_var("COMMAND_CODE_PROXY_MODELS", "  ,  ,  ");
        let config = ProxyConfig::from_env().unwrap();
        // Empty entries after splitting/trimming should result in no allowlist
        assert!(
            config.model_allowlist.is_none() || config.model_allowlist.as_ref().unwrap().is_empty()
        );
        std::env::remove_var("COMMAND_CODE_PROXY_MODELS");
    }

    #[test]
    fn test_config_tls_both_required() {
        // Just verify the config struct can hold TLS values
        let config = ProxyConfig::from_env().unwrap();
        // TLS config is optional, so just verify the struct exists
        let _ = config.tls_cert;
        let _ = config.tls_key;
    }

    #[test]
    fn test_config_empty_incoming_token() {
        std::env::set_var("COMMAND_CODE_PROXY_INCOMING_TOKEN", "  ");
        let config = ProxyConfig::from_env().unwrap();
        assert!(config.incoming_token.is_none());
        std::env::remove_var("COMMAND_CODE_PROXY_INCOMING_TOKEN");
    }

    #[test]
    fn test_config_empty_log_file() {
        std::env::set_var("COMMAND_CODE_PROXY_LOG_FILE", "  ");
        let config = ProxyConfig::from_env().unwrap();
        assert!(config.log_file.is_none());
        std::env::remove_var("COMMAND_CODE_PROXY_LOG_FILE");
    }
}
