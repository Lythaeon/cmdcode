//! `cmdcode connect` — manage upstream provider connections in
//! `~/.cmdcode/providers.json` (opencode-style providers map).

use cmdcode_core::provider_config::ProvidersConfig;
use inquire::{Confirm, Select};
use std::path::PathBuf;

/// Config path used by all connect operations.
fn config_path() -> Result<PathBuf, String> {
    ProvidersConfig::default_path().ok_or_else(|| "cannot determine home directory".into())
}

/// Interactive TUI: `cmdcode connect` (no subcommand). Mirrors `auth`'s
/// loop-with-menu structure.
pub fn tui() {
    loop {
        let cfg = match load_raw() {
            Ok(v) => v,
            Err(e) => fail(&format!("failed to read providers config: {e}")),
        };
        let empty = cfg
            .get("providers")
            .and_then(|p| p.as_object())
            .map(|p| p.is_empty())
            .unwrap_or(true);

        println!();
        if empty {
            println!("no providers configured");
            match Confirm::new("Add an upstream provider now?")
                .with_default(true)
                .prompt()
            {
                Ok(true) => add(),
                _ => return,
            }
            continue;
        }

        // Provider table.
        let Some(providers) = cfg.get("providers").and_then(|p| p.as_object()) else {
            continue;
        };
        println!("Providers ({}):", providers.len());
        for (key, entry) in providers {
            let kind = entry
                .get("type")
                .and_then(|t| t.as_str())
                .unwrap_or("openai");
            let enabled = entry
                .get("enabled")
                .and_then(|e| e.as_bool())
                .unwrap_or(true);
            let model_count = entry
                .get("models")
                .and_then(|m| m.as_object())
                .map(|m| m.len())
                .unwrap_or(0);
            let marker = if enabled { "*" } else { " " };
            let suffix = if enabled { "" } else { "  (disabled)" };
            println!(
                " {marker} {} [{}] {} model(s){suffix}",
                key, kind, model_count
            );
        }
        println!();

        enum Menu {
            Toggle,
            Login,
            Logout,
            Add,
            Remove,
            Test,
            Quit,
        }

        let items = [
            (
                "Sign in a provider (store API key)".to_string(),
                Menu::Login,
            ),
            ("Remove stored key".to_string(), Menu::Logout),
            ("Enable/disable a provider".to_string(), Menu::Toggle),
            ("Add a new provider".to_string(), Menu::Add),
            ("Remove a provider".to_string(), Menu::Remove),
            ("Test connectivity".to_string(), Menu::Test),
            ("Done".to_string(), Menu::Quit),
        ];

        let labels: Vec<String> = items.iter().map(|(l, _)| l.clone()).collect();
        let selection = Select::new("Select an action", labels)
            .with_help_message("↑/↓ to move · Enter to select · Esc to exit")
            .prompt();

        let Ok(chosen_label) = selection else { return };
        let Some((_, action)) = items.iter().find(|(l, _)| l == &chosen_label) else {
            return;
        };
        match action {
            Menu::Toggle => toggle_tui(),
            Menu::Login => match pick_provider("Provider to sign in") {
                Some(name) => login(&name, None),
                None => continue,
            },
            Menu::Logout => match pick_provider("Provider to log out") {
                Some(name) => logout(&name),
                None => continue,
            },
            Menu::Add => add(),
            Menu::Remove => match pick_provider("Provider to remove") {
                Some(name) => remove(&name),
                None => continue,
            },
            Menu::Test => match pick_provider("Provider to test") {
                Some(name) => test(&name),
                None => continue,
            },
            Menu::Quit => return,
        }
    }
}

fn fail(msg: &str) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(1);
}

/// Pick one configured provider id; None on cancel/empty.
fn pick_provider(title: &str) -> Option<String> {
    let cfg = load_raw().ok()?;
    let keys: Vec<String> = cfg
        .get("providers")
        .and_then(|p| p.as_object())
        .map(|p| p.keys().cloned().collect())
        .unwrap_or_default();
    if keys.is_empty() {
        println!("no providers configured");
        return None;
    }
    Select::new(title, keys)
        .with_help_message("↑/↓ to move · Enter to select · Esc to cancel")
        .prompt()
        .ok()
}

/// Toggle flow: pick provider, then choose enable or disable.
fn toggle_tui() {
    let Some(name) = pick_provider("Provider to toggle") else {
        return;
    };

    // Current state determines the default choice ordering.
    let currently_enabled = load_raw().ok().and_then(|cfg| {
        cfg.pointer(&format!("/providers/{name}/enabled"))
            .and_then(|v| v.as_bool())
    });
    let is_enabled = currently_enabled.unwrap_or(true);

    enum Action {
        Enable,
        Disable,
        Cancel,
    }
    let actions: Vec<(&str, Action)> = if is_enabled {
        vec![
            ("Disable", Action::Disable),
            ("Keep enabled", Action::Cancel),
        ]
    } else {
        vec![
            ("Enable", Action::Enable),
            ("Keep disabled", Action::Cancel),
        ]
    };
    let title = format!(
        "{name}: currently {}",
        if is_enabled { "ENABLED" } else { "disabled" }
    );
    let Ok(choice) =
        Select::new(&title, actions.iter().map(|(l, _)| l.to_string()).collect()).prompt()
    else {
        return;
    };
    match actions.iter().find(|(l, _)| l == &choice).map(|(_, a)| a) {
        Some(Action::Enable) => enable(&name),
        Some(Action::Disable) => disable(&name),
        _ => {}
    }
}

/// Load the raw JSON config (creating an empty one if missing).
fn load_raw() -> Result<serde_json::Value, String> {
    let path = config_path()?;
    if let Ok(content) = std::fs::read_to_string(&path) {
        if !content.trim().is_empty() {
            return serde_json::from_str(&content)
                .map_err(|e| format!("parse {}: {e}", path.display()));
        }
    }
    Ok(serde_json::json!({}))
}

/// Persist the raw JSON config with stable ordering and trailing newline.
fn save_raw(value: &serde_json::Value) -> Result<(), String> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    let content = serde_json::to_string_pretty(value).map_err(|e| format!("serialize: {e}"))?;
    std::fs::write(&path, content + "\n").map_err(|e| format!("write {}: {e}", path.display()))
}

/// Show every configured provider, its adapter, base URL and model count.
pub fn list() {
    let cfg = match load_raw() {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "failed to load providers config");
            return;
        }
    };
    let Some(providers) = cfg.get("providers").and_then(|p| p.as_object()) else {
        tracing::info!("no providers configured");
        tracing::info!("add one with: cmdcode connect add");
        return;
    };
    if providers.is_empty() {
        tracing::info!("no providers configured");
        return;
    }

    tracing::info!(count = providers.len(), "configured providers");
    for (key, entry) in providers {
        let kind = entry
            .get("type")
            .and_then(|t| t.as_str())
            .unwrap_or("openai");
        let url = entry
            .pointer("/options/baseURL")
            .and_then(|u| u.as_str())
            .unwrap_or("(no baseURL)");
        let model_count = entry
            .get("models")
            .and_then(|m| m.as_object())
            .map(|m| m.len())
            .unwrap_or(0);
        let learning = entry
            .get("learning")
            .and_then(|l| l.as_bool())
            .unwrap_or(false);
        let enabled = entry
            .get("enabled")
            .and_then(|e| e.as_bool())
            .unwrap_or(true);
        let has_key = cmdcode_core::provider_secrets::ProviderSecretStore::default()
            .get(key)
            .is_some()
            || entry.pointer("/options/apiKey").is_some();
        tracing::info!(
            provider = %key,
            kind,
            url,
            models = model_count,
            learning,
            enabled,
            key = if has_key { "set" } else { "missing" },
            "(declared)"
        );
    }
}

/// Interactive wizard that appends a provider entry to providers.json.
pub fn add() {
    let Ok(name) = inquire::Text::new("Provider id (used in configs, e.g. 'openai')").prompt()
    else {
        return;
    };
    if name.trim().is_empty() {
        tracing::error!("provider id must not be empty");
        return;
    }

    let adapter = inquire::Select::new("Adapter type", vec!["openai", "command-code"]).prompt();

    let Ok(adapter) = adapter.map(|a: &str| a.to_string()) else {
        return;
    };

    let Ok(base_url) = inquire::Text::new(&format!(
        "Base URL{}",
        match adapter.as_str() {
            "openai" => " (include /v1 if required, e.g. https://api.openai.com/v1)",
            _ => "",
        }
    ))
    .with_default(match adapter.as_str() {
        "command-code" => "https://api.commandcode.ai",
        _ => "",
    })
    .prompt() else {
        return;
    };

    let api_key = if adapter == "command-code" {
        // Command Code auth comes from the vault / auth.json.
        None
    } else {
        match inquire::Text::new("API key (blank, or {env:VAR} reference)").prompt() {
            Ok(v) if v.trim().is_empty() => None,
            Ok(v) => Some(v),
            Err(_) => return,
        }
    };

    let Ok(models_raw) =
        inquire::Text::new("Models (comma-separated ids served by this provider)").prompt()
    else {
        return;
    };
    let models: Vec<&str> = models_raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if models.is_empty() {
        tracing::error!("at least one model id is required");
        return;
    }

    let learning = matches!(
        inquire::Confirm::new("Use this provider for taste learning?")
            .with_default(false)
            .prompt(),
        Ok(true)
    );

    // Build the entry as raw JSON to preserve key order and unknown fields.
    let mut options = serde_json::Map::new();
    options.insert(
        "baseURL".into(),
        serde_json::Value::String(base_url.trim().to_string()),
    );
    if let Some(key) = api_key {
        options.insert("apiKey".into(), serde_json::Value::String(key));
    }

    let mut models_map = serde_json::Map::new();
    for id in models {
        models_map.insert(id.to_string(), serde_json::json!({}));
    }

    let mut entry = serde_json::Map::new();
    entry.insert("type".into(), serde_json::Value::String(adapter));
    entry.insert("options".into(), serde_json::Value::Object(options));
    entry.insert("models".into(), serde_json::Value::Object(models_map));
    if learning {
        entry.insert("learning".into(), serde_json::Value::Bool(true));
    }

    let mut cfg = match load_raw() {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "failed to load providers config");
            return;
        }
    };
    if cfg.get("providers").is_none() {
        cfg["providers"] = serde_json::json!({});
    }
    cfg["providers"][&name] = serde_json::Value::Object(entry);

    if let Err(e) = save_raw(&cfg) {
        tracing::error!(error = %e, "failed to save providers config");
        return;
    }

    tracing::info!(provider = %name, "provider added — applies on the next request (hot reload)");
}

/// Remove a provider entry by id.
pub fn remove(name: &str) {
    let mut cfg = match load_raw() {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "failed to load providers config");
            return;
        }
    };
    let removed = cfg
        .get_mut("providers")
        .and_then(|p| p.as_object_mut())
        .and_then(|p| p.remove(name));
    match removed {
        Some(_) => {
            if let Err(e) = save_raw(&cfg) {
                tracing::error!(error = %e, "failed to save providers config");
                return;
            }
            tracing::info!(provider = %name, "provider removed");
        }
        None => tracing::warn!(provider = %name, "not found"),
    }
}

/// Probe a provider's connectivity. For openai adapters hits `{base}/models`
/// with the configured bearer token; for command-code runs a whoami check.
/// Probe a provider's endpoint connectivity (blocking wrapper).
pub fn test(name: &str) {
    match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt.block_on(test_inner(name)),
        Err(e) => tracing::error!(error = %e, "failed to start async runtime"),
    }
}

async fn test_inner(name: &str) {
    let cfg = match load_raw() {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "failed to load providers config");
            return;
        }
    };
    let Some(entry) = cfg.pointer(&format!("/providers/{name}")) else {
        tracing::warn!(provider = %name, "not found");
        return;
    };
    let kind = entry
        .get("type")
        .and_then(|t| t.as_str())
        .unwrap_or("openai");
    let Some(base) = entry
        .pointer("/options/baseURL")
        .and_then(|u| u.as_str())
        .map(|s| s.trim_end_matches('/').to_string())
    else {
        tracing::warn!(provider = %name, "no baseURL configured");
        return;
    };

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "http client error");
            return;
        }
    };

    let url = match kind {
        "command-code" => format!("{base}/alpha/whoami"),
        _ => format!("{base}/models"),
    };

    let mut req = client.get(&url).header("User-Agent", "cli");
    let opt_key = entry.pointer("/options/apiKey").and_then(|k| k.as_str());
    let stored_key = cmdcode_core::provider_secrets::ProviderSecretStore::default().get(name);
    let effective_key = opt_key
        .map(interpolate_env_str)
        .or(stored_key)
        .filter(|k| !k.is_empty());
    if kind == "command-code" && effective_key.is_none() {
        // Fall back to vault/auth.json credentials.
        let store = cmdcode_core::accounts::AccountStore::default();
        let key = store
            .load()
            .ok()
            .and_then(|v| v.active_account().map(|a| a.api_key.as_str().to_string()))
            .or_else(|| {
                let auth_file = dirs::home_dir()?.join(".commandcode").join("auth.json");
                let content = std::fs::read_to_string(auth_file).ok()?;
                let v: serde_json::Value = serde_json::from_str(&content).ok()?;
                v.get("apiKey").and_then(|k| k.as_str()).map(String::from)
            });
        if let Some(key) = key {
            req = req.bearer_auth(key);
        }
    } else if let Some(key) = effective_key {
        match kind {
            "anthropic" => {
                req = req
                    .header("x-api-key", &key)
                    .header("anthropic-version", "2023-06-01");
            }
            "gemini" => {
                req = req.header("x-goog-api-key", &key);
            }
            _ => {
                req = req.bearer_auth(key);
            }
        }
    }

    tracing::info!(provider = %name, url = %url, "probing");
    match req.send().await {
        Ok(resp) => {
            let status = resp.status();
            if status.is_success() {
                tracing::info!(provider = %name, status = status.as_u16(), "reachable");
            } else {
                let body = resp.text().await.unwrap_or_default();
                tracing::error!(
                    provider = %name,
                    status = status.as_u16(),
                    body = %body.chars().take(200).collect::<String>(),
                    "probe failed"
                );
            }
        }
        Err(e) => tracing::error!(provider = %name, error = %e, "unreachable"),
    }
}

/// `{env:VAR}` interpolation for probe-time API keys.
fn interpolate_env_str(value: &str) -> String {
    if !value.contains("{env:") {
        return value.to_string();
    }
    let mut out = String::new();
    let mut rest = value;
    while let Some(start) = rest.find("{env:") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 5..];
        match after.find('}') {
            Some(end) => {
                out.push_str(std::env::var(&after[..end]).unwrap_or_default().as_str());
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// Set the `enabled` flag on a provider entry and persist it. Hot reload
/// applies the change on the next request.
fn set_enabled(name: &str, enabled: bool) {
    let mut cfg = match load_raw() {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "failed to load providers config");
            return;
        }
    };
    let Some(entry) = cfg
        .get_mut("providers")
        .and_then(|p| p.as_object_mut())
        .and_then(|p| p.get_mut(name))
    else {
        tracing::warn!(provider = %name, "not found");
        return;
    };
    if !entry.is_object() {
        tracing::warn!(provider = %name, "entry is not an object");
        return;
    }
    entry["enabled"] = serde_json::Value::Bool(enabled);
    if let Err(e) = save_raw(&cfg) {
        tracing::error!(error = %e, "failed to save providers config");
        return;
    }
    let state = if enabled { "enabled" } else { "disabled" };
    tracing::info!(provider = %name, state, "provider toggled — applies on the next request");
}

/// `cmdcode connect enable <name>`
pub fn enable(name: &str) {
    set_enabled(name, true);
}

/// `cmdcode connect disable <name>`
pub fn disable(name: &str) {
    set_enabled(name, false);
}

/// `cmdcode connect login <name>` — securely store the API key for a
/// provider in `~/.cmdcode/secrets.json` (0600), optionally validating it
/// against the upstream first.
pub fn login(name: &str, key_arg: Option<&str>) {
    let cfg = match load_raw() {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "failed to load providers config");
            return;
        }
    };
    let Some(entry) = cfg.pointer(&format!("/providers/{name}")) else {
        tracing::warn!(provider = %name, "not found — add it first with connect add");
        return;
    };
    let kind = entry
        .get("type")
        .and_then(|t| t.as_str())
        .unwrap_or("openai");
    if kind == "command-code" {
        tracing::info!("command-code uses the account vault — run 'cmdcode auth' instead");
        return;
    }

    let key = match key_arg {
        Some(k) if !k.trim().is_empty() => k.to_string(),
        Some(_) => {
            tracing::error!("key must not be empty");
            return;
        }
        None => match inquire::Password::new(&format!("API key for {name}"))
            .with_help_message("Input is masked; press Esc to cancel")
            .prompt()
        {
            Ok(k) if !k.trim().is_empty() => k,
            Ok(_) => {
                tracing::error!("key must not be empty");
                return;
            }
            Err(e) => {
                tracing::error!(error = %e, "cancelled");
                return;
            }
        },
    };

    // Validate before saving when the endpoint exposes a probe.
    let base = entry
        .pointer("/options/baseURL")
        .and_then(|u| u.as_str())
        .map(|s| s.trim_end_matches('/').to_string());
    if let Some(base) = base {
        let url = match kind {
            "anthropic" => format!("{base}/v1/models"),
            _ => format!("{base}/models"),
        };
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(error = %e, "failed to start async runtime");
                return;
            }
        };
        let valid = rt.block_on(async {
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .ok()?;
            let mut req = client.get(&url);
            match kind {
                "anthropic" => {
                    req = req
                        .header("x-api-key", key.trim())
                        .header("anthropic-version", "2023-06-01");
                }
                "gemini" => {
                    req = req.header("x-goog-api-key", key.trim());
                }
                _ => {
                    req = req.bearer_auth(key.trim());
                }
            }
            let resp = req.send().await.ok()?;
            Some(resp.status().is_success())
        });
        match valid {
            Some(true) => tracing::info!(provider = %name, "key validated"),
            Some(false) => {
                // Key may still be valid for chat-only endpoints; warn but save.
                tracing::warn!(
                    provider = %name,
                    "key did not validate against the models endpoint; saving anyway"
                );
            }
            None => {
                tracing::warn!(provider = %name, "could not reach upstream; saving anyway");
            }
        }
    }

    let store = cmdcode_core::provider_secrets::ProviderSecretStore::default();
    if let Err(e) = store.set(name, key.trim()) {
        tracing::error!(error = %e, "failed to save key");
        return;
    }
    tracing::info!(
        provider = %name,
        path = %cmdcode_core::provider_secrets::ProviderSecretStore::default_path()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        "key stored (chmod 0600)"
    );
}

/// `cmdcode connect logout <name>` — remove the stored key.
pub fn logout(name: &str) {
    let store = cmdcode_core::provider_secrets::ProviderSecretStore::default();
    match store.remove(name) {
        Ok(true) => tracing::info!(provider = %name, "stored key removed"),
        Ok(false) => tracing::warn!(provider = %name, "no stored key found"),
        Err(e) => tracing::error!(error = %e, "failed to remove key"),
    }
}
