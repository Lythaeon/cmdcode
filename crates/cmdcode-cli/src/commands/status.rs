use cmdcode_core::model_catalog::get_model_catalog;
use std::path::PathBuf;

/// Sanitize a path for display by replacing the home directory with ~.
fn sanitize_path(path: &PathBuf) -> String {
    let home = dirs::home_dir().unwrap_or_default();
    if let Ok(stripped) = path.strip_prefix(&home) {
        format!("~/{}", stripped.display())
    } else {
        path.display().to_string()
    }
}

pub fn run() {
    let auth_dir = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".commandcode");
    let auth_file = auth_dir.join("auth.json");

    tracing::info!("checking status");

    // Check auth file
    if !auth_file.exists() {
        tracing::error!(
            path = %sanitize_path(&auth_file),
            "auth.json not found"
        );
        tracing::info!(
            "install command-code CLI and run: command-code login"
        );
        std::process::exit(1);
    }

    let content = match std::fs::read_to_string(&auth_file) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "failed to read auth.json");
            std::process::exit(1);
        }
    };

    let auth: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "invalid auth.json");
            std::process::exit(1);
        }
    };

    let has_api_key = auth
        .get("apiKey")
        .and_then(|v| v.as_str())
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    let has_oauth = auth
        .get("oauthToken")
        .and_then(|v| v.as_str())
        .map(|s| !s.is_empty())
        .unwrap_or(false);

    if !has_api_key && !has_oauth {
        tracing::error!("no credentials in auth.json (need apiKey or oauthToken)");
        tracing::info!("run: command-code login");
        std::process::exit(1);
    }

    let method = if has_api_key { "API key" } else { "OAuth" };
    tracing::info!(method = %method, "authentication OK");

    // Check model catalog
    let catalog = get_model_catalog();
    let count = catalog.len();
    tracing::info!(count = count, "models available");

    // Check config
    let config_file = auth_dir.join("config.json");
    if config_file.exists() {
        tracing::info!(path = %sanitize_path(&config_file), "config file found");
    } else {
        tracing::info!("config: using defaults");
    }

    tracing::info!("proxy ready, start with: cmdcode serve");
}
