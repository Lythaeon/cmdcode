use proxy_core::auth::AuthManager;
use proxy_core::config::ProxyConfig;
use proxy_server::ProxyService;

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = ProxyConfig::from_env()?;

    // Init tracing
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&config.log_level));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_thread_ids(true)
        .init();

    let auth = AuthManager::new(config.auth_dir.clone(), config.auth_cache_ttl_secs);

    tracing::info!(
        listen = %config.listen_addr,
        upstream = %config.upstream_url,
        timeout = config.upstream_timeout_secs,
        retries = config.max_retries,
        default_model = %config.default_model,
        "starting command-code-proxy"
    );

    let service = ProxyService::new(config, auth);
    service.run()
}
