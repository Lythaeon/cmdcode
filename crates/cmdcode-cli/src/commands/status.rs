use cmdcode_core::model_catalog::get_model_catalog;
use std::path::PathBuf;

pub fn run() {
    let auth_dir = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".commandcode");
    let auth_file = auth_dir.join("auth.json");

    println!("cmdcode status\n");

    // Check auth file
    if !auth_file.exists() {
        eprintln!("error: auth.json not found at {}", auth_file.display());
        eprintln!();
        eprintln!("The command-code CLI is a hard dependency. Install and log in:");
        eprintln!();
        eprintln!("  npm install -g command-code");
        eprintln!("  command-code login");
        eprintln!();
        std::process::exit(1);
    }

    let content = match std::fs::read_to_string(&auth_file) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: failed to read auth.json: {e}");
            std::process::exit(1);
        }
    };

    let auth: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error: invalid auth.json: {e}");
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
        eprintln!("error: no credentials in auth.json (need apiKey or oauthToken)");
        eprintln!();
        eprintln!("Run: command-code login");
        std::process::exit(1);
    }

    let method = if has_api_key { "API key" } else { "OAuth" };
    println!("auth:     OK ({method})");

    // Check model catalog
    let catalog = get_model_catalog();
    let count = catalog.len();
    println!("models:   {count} available");

    // Check config
    let config_file = auth_dir.join("config.json");
    if config_file.exists() {
        println!("config:   {}", config_file.display());
    } else {
        println!("config:   (using defaults)");
    }

    println!();
    println!("proxy is ready. start with: cmdcode serve");
}
