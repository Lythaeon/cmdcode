use std::path::{Path, PathBuf};

/// Sanitize a path for display by replacing the home directory with ~.
fn sanitize_path(path: &Path) -> String {
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

    tracing::info!("checking authentication");

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

    tracing::info!(
        auth_dir = %sanitize_path(&auth_dir),
        auth_file = %sanitize_path(&auth_file),
        "auth configuration"
    );

    // API key
    let has_api_key = auth
        .get("apiKey")
        .and_then(|v| v.as_str())
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    if has_api_key {
        let key = auth.get("apiKey").and_then(|v| v.as_str()).unwrap_or("");
        let masked = mask_credential(key);
        tracing::info!(api_key = %masked, "API key configured");
    } else {
        tracing::info!("api_key: not set");
    }

    // OAuth
    let has_oauth = auth
        .get("oauthToken")
        .and_then(|v| v.as_str())
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    if has_oauth {
        let token = auth
            .get("oauthToken")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let masked = mask_credential(token);
        let provider = auth
            .get("oauthProvider")
            .and_then(|v| v.as_str())
            .unwrap_or("(unknown)");
        tracing::info!(oauth_token = %masked, provider = %provider, "OAuth configured");
    } else {
        tracing::info!("oauth_token: not set");
    }

    // User info
    if let Some(user_id) = auth.get("userId").and_then(|v| v.as_str()) {
        if !user_id.is_empty() {
            tracing::info!(user_id = %user_id);
        }
    }
    if let Some(user_name) = auth.get("userName").and_then(|v| v.as_str()) {
        if !user_name.is_empty() {
            tracing::info!(user_name = %user_name);
        }
    }

    // Config file
    let config_file = auth_dir.join("config.json");
    if config_file.exists() {
        tracing::info!(path = %sanitize_path(&config_file), "config file found");
        if let Ok(config_content) = std::fs::read_to_string(&config_file) {
            if let Ok(config) = serde_json::from_str::<serde_json::Value>(&config_content) {
                if let Some(model) = config.get("model").and_then(|v| v.as_str()) {
                    tracing::info!(preferred_model = %model);
                }
                if let Some(taste) = config.get("tasteLearning").and_then(|v| v.as_bool()) {
                    tracing::info!(taste_learning = %taste);
                }
                if let Some(oauth) = config.get("oauthEnforced").and_then(|v| v.as_bool()) {
                    tracing::info!(oauth_enforced = %oauth);
                }
            }
        }
    }

    if has_api_key || has_oauth {
        tracing::info!("authentication: OK");
    } else {
        tracing::error!("authentication: NO CREDENTIALS - run: command-code login");
        std::process::exit(1);
    }
}

/// Mask a credential string, showing only the first 4 and last 4 characters.
fn mask_credential(s: &str) -> String {
    if s.len() <= 8 {
        return "*".repeat(s.len());
    }
    let prefix = &s[..4];
    let suffix = &s[s.len() - 4..];
    let masked_len = s.len() - 8;
    format!("{prefix}{}{suffix}", "*".repeat(masked_len))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn test_mask_short_credential() {
        assert_eq!(mask_credential("abc"), "***");
        assert_eq!(mask_credential("12345678"), "********");
    }

    #[test]
    fn test_mask_long_credential() {
        let masked = mask_credential("sk-1234567890abcdef");
        assert_eq!(masked, "sk-1***********cdef");
        assert!(masked.starts_with("sk-1"));
        assert!(masked.ends_with("cdef"));
    }

    #[test]
    fn test_mask_exactly_8() {
        assert_eq!(mask_credential("12345678"), "********");
    }

    #[test]
    fn test_mask_9_chars() {
        let masked = mask_credential("123456789");
        assert_eq!(masked, "1234*6789");
    }
}
