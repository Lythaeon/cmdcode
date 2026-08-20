use std::path::PathBuf;

pub fn run() {
    let auth_dir = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".commandcode");
    let auth_file = auth_dir.join("auth.json");

    println!("cmdcode auth\n");

    if !auth_file.exists() {
        eprintln!("error: auth.json not found at {}", auth_file.display());
        eprintln!();
        eprintln!("The command-code CLI is a hard dependency. Install and log in:");
        eprintln!();
        eprintln!("  npm install -g command-code");
        eprintln!("  command-code login");
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

    println!("auth_dir:    {}", auth_dir.display());
    println!("auth_file:   {}", auth_file.display());

    // API key
    let has_api_key = auth
        .get("apiKey")
        .and_then(|v| v.as_str())
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    if has_api_key {
        let key = auth.get("apiKey").and_then(|v| v.as_str()).unwrap_or("");
        let masked = mask_credential(key);
        println!("api_key:     {masked}");
    } else {
        println!("api_key:     (not set)");
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
        println!("oauth_token: {masked}");
        let provider = auth
            .get("oauthProvider")
            .and_then(|v| v.as_str())
            .unwrap_or("(unknown)");
        println!("oauth_provider: {provider}");
    } else {
        println!("oauth_token: (not set)");
    }

    // User info
    if let Some(user_id) = auth.get("userId").and_then(|v| v.as_str()) {
        if !user_id.is_empty() {
            println!("user_id:     {user_id}");
        }
    }
    if let Some(user_name) = auth.get("userName").and_then(|v| v.as_str()) {
        if !user_name.is_empty() {
            println!("user_name:   {user_name}");
        }
    }

    // Config file
    let config_file = auth_dir.join("config.json");
    if config_file.exists() {
        println!();
        println!("config_file: {}", config_file.display());
        if let Ok(config_content) = std::fs::read_to_string(&config_file) {
            if let Ok(config) = serde_json::from_str::<serde_json::Value>(&config_content) {
                if let Some(model) = config.get("model").and_then(|v| v.as_str()) {
                    println!("preferred_model: {model}");
                }
                if let Some(taste) = config.get("tasteLearning").and_then(|v| v.as_bool()) {
                    println!("taste_learning:  {taste}");
                }
                if let Some(oauth) = config.get("oauthEnforced").and_then(|v| v.as_bool()) {
                    println!("oauth_enforced:  {oauth}");
                }
            }
        }
    }

    println!();
    if has_api_key || has_oauth {
        println!("authentication: OK");
    } else {
        eprintln!("authentication: NO CREDENTIALS");
        eprintln!();
        eprintln!("Run: command-code login");
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
