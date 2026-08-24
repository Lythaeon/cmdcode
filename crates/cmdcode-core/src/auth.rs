use crate::accounts::AccountStore;
use crate::error::AuthError;
use crate::types::{CliEnvironment, SensitiveString, SessionId};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Cached CLI version — spawning `command-code --version` costs ~600ms
/// (Node CLI), so this must run at most once per process, not per request.
static CLI_VERSION: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();

/// Detect the installed command-code CLI version by running `command-code --version`.
/// Returns None if the CLI is not installed or the version cannot be parsed.
fn detect_cli_version() -> Option<String> {
    CLI_VERSION
        .get_or_init(|| {
            let output = std::process::Command::new("command-code")
                .arg("--version")
                .output()
                .ok()?;
            let stdout = String::from_utf8_lossy(&output.stdout);
            // Parse "1.32.1" from "1.32.1" or "command-code 1.32.1" etc.
            let ver = stdout.split_whitespace().last()?;
            if ver.chars().any(|c| c.is_ascii_digit() || c == '.') && ver.contains('.') {
                Some(ver.to_string())
            } else {
                None
            }
        })
        .clone()
}

/// Raw auth file contents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthData {
    /// API key for upstream authentication.
    #[serde(default, alias = "apiKey")]
    pub api_key: Option<SensitiveString>,
    /// OAuth token for upstream authentication.
    #[serde(default, alias = "oauthToken")]
    pub oauth_token: Option<SensitiveString>,
    /// OAuth provider name.
    #[serde(default, alias = "oauthProvider")]
    pub oauth_provider: Option<String>,
    /// User identifier.
    #[serde(default, alias = "userId")]
    pub user_id: Option<String>,
    /// Human-readable user name.
    #[serde(default, alias = "userName")]
    pub user_name: Option<String>,
}

/// Raw config file contents.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConfigData {
    /// Preferred model name.
    #[serde(default)]
    pub model: Option<String>,
    /// Whether taste-learning is enabled.
    #[serde(default, alias = "tasteLearning")]
    pub taste_learning: Option<bool>,
    /// Whether OAuth is enforced.
    #[serde(default, alias = "oauthEnforced")]
    pub oauth_enforced: Option<bool>,
}

/// Authentication method.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthMethod {
    /// API key authentication.
    ApiKey(SensitiveString),
    /// OAuth token authentication.
    OAuth {
        /// OAuth access token.
        token: SensitiveString,
        /// OAuth provider name.
        provider: String,
    },
}

/// Cached auth state with TTL.
#[derive(Debug)]
struct CachedAuth {
    #[allow(dead_code)]
    auth: AuthData,
    config: ConfigData,
    method: Option<AuthMethod>,
    last_read: Instant,
    ttl: Duration,
}

impl CachedAuth {
    fn is_expired(&self) -> bool {
        self.last_read.elapsed() > self.ttl
    }
}

/// Thread-safe auth manager with cached reads.
#[derive(Clone)]
pub struct AuthManager {
    auth_dir: PathBuf,
    cache_ttl: Duration,
    /// Optional multi-account vault. When present, the active account's
    /// credential is used as the authentication method.
    store: Option<AccountStore>,
    state: Arc<RwLock<Option<CachedAuth>>>,
}

impl AuthManager {
    /// Create a new auth manager that reads credentials from `auth_dir`.
    pub fn new(auth_dir: PathBuf, cache_ttl_secs: u64) -> Self {
        Self {
            auth_dir,
            cache_ttl: Duration::from_secs(cache_ttl_secs),
            store: None,
            state: Arc::new(RwLock::new(None)),
        }
    }

    /// Create an auth manager backed by the multi-account vault. The active
    /// vault account supplies the credential; legacy `auth.json` is used only
    /// as a fallback when the vault has no active account.
    pub fn with_vault(auth_dir: PathBuf, cache_ttl_secs: u64, store: AccountStore) -> Self {
        Self {
            auth_dir,
            cache_ttl: Duration::from_secs(cache_ttl_secs),
            store: Some(store),
            state: Arc::new(RwLock::new(None)),
        }
    }

    /// Whether this manager is backed by the multi-account vault.
    pub fn has_vault(&self) -> bool {
        self.store.is_some()
    }

    /// Whether auto-rotate is enabled in the vault settings.
    pub async fn auto_rotate_enabled(&self) -> bool {
        let Some(store) = &self.store else {
            return false;
        };
        match store.load() {
            Ok(v) => v.settings.auto_rotate,
            Err(_) => false,
        }
    }

    /// Set the vault's auto-rotate setting.
    pub async fn set_auto_rotate(&self, enabled: bool) -> Result<(), AuthError> {
        let Some(store) = &self.store else {
            return Err(AuthError::NoAuthConfigured);
        };
        let mut vault = store.load()?;
        vault.settings.auto_rotate = enabled;
        store.save(&vault)
    }

    /// Rotate to the next account in the vault. Returns the newly active
    /// account's display name, or `None` when there is nothing to rotate to.
    /// Persists the new active pointer and drops any cached credential so the
    /// next request uses the new account.
    pub async fn rotate_to_next(&self) -> Result<Option<String>, AuthError> {
        let Some(store) = &self.store else {
            return Ok(None);
        };
        let mut vault = store.load()?;
        let next = vault.rotate_next();
        if next.is_none() {
            return Ok(None);
        }
        store.save(&vault)?;
        self.invalidate_cache().await;
        Ok(vault.active_account().map(|a| a.display_name().to_string()))
    }

    /// Handle an upstream auth rejection: rotate to the next account when
    /// auto-rotate is enabled and a second account exists, otherwise just
    /// invalidate so the credential is re-read from disk.
    pub async fn on_auth_rejected(&self) -> Option<String> {
        if self.auto_rotate_enabled().await {
            self.rotate_to_next().await.ok().flatten()
        } else {
            self.invalidate_cache().await;
            None
        }
    }

    /// Returns the cached auth method, refreshing from disk if expired.
    pub async fn get_auth_method(&self) -> Result<AuthMethod, AuthError> {
        let needs_refresh = {
            let state = self.state.read().await;
            state.as_ref().is_none_or(|c| c.is_expired())
        };

        if needs_refresh {
            self.refresh().await?;
        }

        let state = self.state.read().await;
        state
            .as_ref()
            .and_then(|c| c.method.clone())
            .ok_or(AuthError::NoAuthConfigured)
    }

    /// Returns the cached config data, refreshing from disk if expired.
    pub async fn get_config(&self) -> ConfigData {
        let needs_refresh = {
            let state = self.state.read().await;
            state.as_ref().is_none_or(|c| c.is_expired())
        };

        if needs_refresh {
            let _ = self.refresh().await;
        }

        let state = self.state.read().await;
        state.as_ref().map(|c| c.config.clone()).unwrap_or_default()
    }

    /// Drop the cached auth so the next read re-reads `auth.json`.
    /// Used when the upstream rejects a credential (401/403) so the proxy
    /// does not keep using a stale cached key until the TTL expires.
    pub async fn invalidate_cache(&self) {
        let mut state = self.state.write().await;
        *state = None;
    }

    async fn refresh(&self) -> Result<(), AuthError> {
        let (auth, config) = self.load_credentials().await?;

        let method = if let Some(ref key) = auth.api_key {
            Some(AuthMethod::ApiKey(key.clone()))
        } else if let Some(ref token) = auth.oauth_token {
            Some(AuthMethod::OAuth {
                token: token.clone(),
                provider: auth.oauth_provider.clone().unwrap_or_default(),
            })
        } else {
            None
        };

        let cached = CachedAuth {
            auth,
            config,
            method,
            last_read: Instant::now(),
            ttl: self.cache_ttl,
        };

        let mut state = self.state.write().await;
        *state = Some(cached);

        Ok(())
    }

    /// Load raw auth data and config, preferring the vault's active account
    /// when present and falling back to the legacy `auth.json`.
    async fn load_credentials(&self) -> Result<(AuthData, ConfigData), AuthError> {
        if let Some(store) = &self.store {
            if let Ok(vault) = store.load() {
                if let Some(active) = vault.active_account() {
                    let config = self.read_config().await;
                    let auth = AuthData {
                        api_key: Some(active.api_key.clone()),
                        oauth_token: None,
                        oauth_provider: None,
                        user_id: Some(active.user_id.clone()),
                        user_name: Some(active.user_name.clone()),
                    };
                    return Ok((auth, config));
                }
            }
        }

        // Fall back to the legacy `auth.json`.
        let auth_file = self.auth_dir.join("auth.json");
        if !auth_file.exists() {
            return Err(AuthError::FileNotFound {
                path: auth_file.display().to_string(),
            });
        }

        let auth_content =
            tokio::fs::read_to_string(&auth_file)
                .await
                .map_err(|e| AuthError::FileNotFound {
                    path: format!("{}: {e}", auth_file.display()),
                })?;

        let auth: AuthData =
            serde_json::from_str(&auth_content).map_err(|e| AuthError::InvalidJson {
                path: auth_file.display().to_string(),
                source: e,
            })?;

        let config = self.read_config().await;
        Ok((auth, config))
    }

    async fn read_config(&self) -> ConfigData {
        let config_file = self.auth_dir.join("config.json");
        if config_file.exists() {
            tokio::fs::read_to_string(&config_file)
                .await
                .ok()
                .and_then(|c| serde_json::from_str(&c).ok())
                .unwrap_or_default()
        } else {
            ConfigData::default()
        }
    }

    /// Build HTTP headers matching the CLI fingerprint.
    pub async fn build_headers(&self, cwd: &str) -> Result<HashMap<String, String>, AuthError> {
        let method = self.get_auth_method().await?;
        let config = self.get_config().await;
        let session_id = SessionId::generate();

        let project_slug = std::path::Path::new(cwd)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        let mut headers = HashMap::new();
        headers.insert("Content-Type".into(), "application/json".into());
        headers.insert("User-Agent".into(), "cli".into());
        headers.insert(
            "x-command-code-version".into(),
            detect_cli_version().unwrap_or_else(|| "1.32.1".into()),
        );
        // Sanitize env var value to prevent header injection via CRLF.
        let cli_env_str = std::env::var("COMMAND_CODE_ENV")
            .unwrap_or_else(|_| "production".into())
            .chars()
            .filter(|c| *c != '\r' && *c != '\n')
            .collect::<String>();
        let cli_env =
            CliEnvironment::from_str_opt(&cli_env_str).unwrap_or(CliEnvironment::Production);
        headers.insert("x-cli-environment".into(), cli_env.as_str().into());
        headers.insert("x-project-slug".into(), project_slug.into());
        headers.insert(
            "x-taste-learning".into(),
            config.taste_learning.unwrap_or(true).to_string(),
        );
        headers.insert(
            "x-co-flag".into(),
            config.oauth_enforced.unwrap_or(false).to_string(),
        );
        headers.insert("x-session-id".into(), session_id.as_str().into());

        match method {
            AuthMethod::ApiKey(key) => {
                headers.insert("Authorization".into(), format!("Bearer {}", key.as_str()));
            }
            AuthMethod::OAuth { token, provider } => {
                headers.insert("x-oauth-token".into(), format!("Bearer {}", token.as_str()));
                if !provider.is_empty() {
                    headers.insert("x-oauth-provider".into(), provider);
                }
            }
        }

        Ok(headers)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_auth_manager_loads_key() {
        let tmp = TempDir::new().unwrap();
        let auth_dir = tmp.path().join(".commandcode");
        std::fs::create_dir_all(&auth_dir).unwrap();
        std::fs::write(
            auth_dir.join("auth.json"),
            r#"{"apiKey":"test-key-12345678"}"#,
        )
        .unwrap();

        let mgr = AuthManager::new(auth_dir, 30);
        let method = mgr.get_auth_method().await.unwrap();
        match method {
            AuthMethod::ApiKey(k) => assert_eq!(k.as_str(), "test-key-12345678"),
            _ => panic!("expected API key"),
        }
    }

    #[tokio::test]
    async fn test_auth_manager_missing_file() {
        let tmp = TempDir::new().unwrap();
        let auth_dir = tmp.path().join(".commandcode");
        std::fs::create_dir_all(&auth_dir).unwrap();

        let mgr = AuthManager::new(auth_dir, 30);
        let err = mgr.get_auth_method().await.unwrap_err();
        assert!(matches!(err, AuthError::FileNotFound { .. }));
    }

    #[tokio::test]
    async fn test_auth_manager_no_auth_configured() {
        let tmp = TempDir::new().unwrap();
        let auth_dir = tmp.path().join(".commandcode");
        std::fs::create_dir_all(&auth_dir).unwrap();
        std::fs::write(auth_dir.join("auth.json"), r#"{}"#).unwrap();

        let mgr = AuthManager::new(auth_dir, 30);
        let err = mgr.get_auth_method().await.unwrap_err();
        assert!(matches!(err, AuthError::NoAuthConfigured));
    }

    #[tokio::test]
    async fn test_auth_manager_caches() {
        let tmp = TempDir::new().unwrap();
        let auth_dir = tmp.path().join(".commandcode");
        std::fs::create_dir_all(&auth_dir).unwrap();
        std::fs::write(auth_dir.join("auth.json"), r#"{"apiKey":"key1"}"#).unwrap();

        let mgr = AuthManager::new(auth_dir.clone(), 60);
        let _ = mgr.get_auth_method().await.unwrap();

        // Change file — should not be visible until cache expires
        std::fs::write(auth_dir.join("auth.json"), r#"{"apiKey":"key2"}"#).unwrap();

        let method = mgr.get_auth_method().await.unwrap();
        match method {
            AuthMethod::ApiKey(k) => assert_eq!(k.as_str(), "key1"), // cached
            _ => panic!("expected cached key"),
        }
    }

    #[tokio::test]
    async fn test_build_headers() {
        let tmp = TempDir::new().unwrap();
        let auth_dir = tmp.path().join(".commandcode");
        std::fs::create_dir_all(&auth_dir).unwrap();
        std::fs::write(auth_dir.join("auth.json"), r#"{"apiKey":"my-api-key"}"#).unwrap();

        let mgr = AuthManager::new(auth_dir, 30);
        let headers = mgr.build_headers("/tmp/test").await.unwrap();

        assert_eq!(headers.get("Authorization").unwrap(), "Bearer my-api-key");
        assert_eq!(headers.get("User-Agent").unwrap(), "cli");
        assert!(headers.contains_key("x-session-id"));
        assert_eq!(headers.get("x-project-slug").unwrap(), "test");
    }

    // === Security-focused tests ===

    #[tokio::test]
    async fn test_auth_manager_invalid_json() {
        let tmp = TempDir::new().unwrap();
        let auth_dir = tmp.path().join(".commandcode");
        std::fs::create_dir_all(&auth_dir).unwrap();
        std::fs::write(auth_dir.join("auth.json"), r#"not json"#).unwrap();

        let mgr = AuthManager::new(auth_dir, 30);
        let err = mgr.get_auth_method().await.unwrap_err();
        assert!(matches!(err, AuthError::InvalidJson { .. }));
    }

    #[tokio::test]
    async fn test_auth_manager_empty_api_key() {
        let tmp = TempDir::new().unwrap();
        let auth_dir = tmp.path().join(".commandcode");
        std::fs::create_dir_all(&auth_dir).unwrap();
        std::fs::write(auth_dir.join("auth.json"), r#"{"apiKey":""}"#).unwrap();

        let mgr = AuthManager::new(auth_dir, 30);
        // Empty API key is still returned as ApiKey("") - the proxy handles this
        let method = mgr.get_auth_method().await.unwrap();
        match method {
            AuthMethod::ApiKey(k) => assert!(k.is_empty()),
            _ => panic!("expected ApiKey"),
        }
    }

    #[tokio::test]
    async fn test_auth_manager_empty_oauth_token() {
        let tmp = TempDir::new().unwrap();
        let auth_dir = tmp.path().join(".commandcode");
        std::fs::create_dir_all(&auth_dir).unwrap();
        std::fs::write(auth_dir.join("auth.json"), r#"{"oauthToken":""}"#).unwrap();

        let mgr = AuthManager::new(auth_dir, 30);
        // Empty OAuth token is still returned as OAuth with empty token
        let method = mgr.get_auth_method().await.unwrap();
        match method {
            AuthMethod::OAuth { token, .. } => assert!(token.is_empty()),
            _ => panic!("expected OAuth"),
        }
    }

    #[tokio::test]
    async fn test_auth_manager_oauth_with_provider() {
        let tmp = TempDir::new().unwrap();
        let auth_dir = tmp.path().join(".commandcode");
        std::fs::create_dir_all(&auth_dir).unwrap();
        std::fs::write(
            auth_dir.join("auth.json"),
            r#"{"oauthToken":"tok123","oauthProvider":"github"}"#,
        )
        .unwrap();

        let mgr = AuthManager::new(auth_dir, 30);
        let method = mgr.get_auth_method().await.unwrap();
        match method {
            AuthMethod::OAuth { token, provider } => {
                assert_eq!(token.as_str(), "tok123");
                assert_eq!(provider, "github");
            }
            _ => panic!("expected OAuth"),
        }
    }

    #[tokio::test]
    async fn test_auth_manager_oauth_without_provider() {
        let tmp = TempDir::new().unwrap();
        let auth_dir = tmp.path().join(".commandcode");
        std::fs::create_dir_all(&auth_dir).unwrap();
        std::fs::write(auth_dir.join("auth.json"), r#"{"oauthToken":"tok456"}"#).unwrap();

        let mgr = AuthManager::new(auth_dir, 30);
        let method = mgr.get_auth_method().await.unwrap();
        match method {
            AuthMethod::OAuth { token, provider } => {
                assert_eq!(token.as_str(), "tok456");
                assert!(provider.is_empty());
            }
            _ => panic!("expected OAuth"),
        }
    }

    #[tokio::test]
    async fn test_auth_manager_invalidate_cache() {
        let tmp = TempDir::new().unwrap();
        let auth_dir = tmp.path().join(".commandcode");
        std::fs::create_dir_all(&auth_dir).unwrap();
        std::fs::write(auth_dir.join("auth.json"), r#"{"apiKey":"key1"}"#).unwrap();

        let mgr = AuthManager::new(auth_dir.clone(), 60);
        let _ = mgr.get_auth_method().await.unwrap();

        // Invalidate and change file
        mgr.invalidate_cache().await;
        std::fs::write(auth_dir.join("auth.json"), r#"{"apiKey":"key2"}"#).unwrap();

        let method = mgr.get_auth_method().await.unwrap();
        match method {
            AuthMethod::ApiKey(k) => assert_eq!(k.as_str(), "key2"), // should see new key
            _ => panic!("expected new key"),
        }
    }

    #[tokio::test]
    async fn test_build_headers_camel_case_aliases() {
        let tmp = TempDir::new().unwrap();
        let auth_dir = tmp.path().join(".commandcode");
        std::fs::create_dir_all(&auth_dir).unwrap();
        // Test camelCase JSON field aliases
        std::fs::write(
            auth_dir.join("auth.json"),
            r#"{"apiKey":"key-camel","oauthToken":"tok-camel","oauthProvider":"gitlab","userId":"u1","userName":"test-user"}"#,
        )
        .unwrap();

        let mgr = AuthManager::new(auth_dir, 30);
        let headers = mgr.build_headers("/tmp/test").await.unwrap();
        assert_eq!(headers.get("Authorization").unwrap(), "Bearer key-camel");
    }

    #[tokio::test]
    async fn test_build_headers_config_file() {
        let tmp = TempDir::new().unwrap();
        let auth_dir = tmp.path().join(".commandcode");
        std::fs::create_dir_all(&auth_dir).unwrap();
        std::fs::write(auth_dir.join("auth.json"), r#"{"apiKey":"k"}"#).unwrap();
        std::fs::write(
            auth_dir.join("config.json"),
            r#"{"tasteLearning":false,"oauthEnforced":true}"#,
        )
        .unwrap();

        let mgr = AuthManager::new(auth_dir, 30);
        let headers = mgr.build_headers("/tmp/test").await.unwrap();
        assert_eq!(headers.get("x-taste-learning").unwrap(), "false");
        assert_eq!(headers.get("x-co-flag").unwrap(), "true");
    }

    #[tokio::test]
    async fn test_build_headers_missing_config_file() {
        let tmp = TempDir::new().unwrap();
        let auth_dir = tmp.path().join(".commandcode");
        std::fs::create_dir_all(&auth_dir).unwrap();
        std::fs::write(auth_dir.join("auth.json"), r#"{"apiKey":"k"}"#).unwrap();
        // No config.json

        let mgr = AuthManager::new(auth_dir, 30);
        let headers = mgr.build_headers("/tmp/test").await.unwrap();
        // Should use defaults
        assert_eq!(headers.get("x-taste-learning").unwrap(), "true");
        assert_eq!(headers.get("x-co-flag").unwrap(), "false");
    }

    #[tokio::test]
    async fn test_build_headers_empty_project_slug() {
        let tmp = TempDir::new().unwrap();
        let auth_dir = tmp.path().join(".commandcode");
        std::fs::create_dir_all(&auth_dir).unwrap();
        std::fs::write(auth_dir.join("auth.json"), r#"{"apiKey":"k"}"#).unwrap();

        let mgr = AuthManager::new(auth_dir, 30);
        // Root path with no file_name
        let headers = mgr.build_headers("/").await.unwrap();
        assert_eq!(headers.get("x-project-slug").unwrap(), "unknown");
    }

    #[tokio::test]
    async fn test_auth_data_parse_camel_case() {
        let json = r#"{"apiKey":"ak","oauthToken":"ot","oauthProvider":"p","userId":"uid","userName":"un"}"#;
        let auth: AuthData = serde_json::from_str(json).unwrap();
        assert_eq!(auth.api_key.as_deref(), Some("ak"));
        assert_eq!(auth.oauth_token.as_deref(), Some("ot"));
        assert_eq!(auth.oauth_provider.as_deref(), Some("p"));
        assert_eq!(auth.user_id.as_deref(), Some("uid"));
        assert_eq!(auth.user_name.as_deref(), Some("un"));
    }

    #[tokio::test]
    async fn test_auth_data_parse_snake_case() {
        let json = r#"{"api_key":"ak","oauth_token":"ot","oauth_provider":"p","user_id":"uid","user_name":"un"}"#;
        let auth: AuthData = serde_json::from_str(json).unwrap();
        assert_eq!(auth.api_key.as_deref(), Some("ak"));
        assert_eq!(auth.oauth_token.as_deref(), Some("ot"));
        assert_eq!(auth.oauth_provider.as_deref(), Some("p"));
        assert_eq!(auth.user_id.as_deref(), Some("uid"));
        assert_eq!(auth.user_name.as_deref(), Some("un"));
    }

    #[tokio::test]
    async fn test_auth_data_parse_empty() {
        let json = r#"{}"#;
        let auth: AuthData = serde_json::from_str(json).unwrap();
        assert!(auth.api_key.is_none());
        assert!(auth.oauth_token.is_none());
        assert!(auth.oauth_provider.is_none());
        assert!(auth.user_id.is_none());
        assert!(auth.user_name.is_none());
    }

    #[tokio::test]
    async fn test_config_data_defaults() {
        let config = ConfigData::default();
        assert!(config.model.is_none());
        assert!(config.taste_learning.is_none());
        assert!(config.oauth_enforced.is_none());
    }

    #[tokio::test]
    async fn test_config_data_parse() {
        let json = r#"{"model":"gpt-4","tasteLearning":false,"oauthEnforced":true}"#;
        let config: ConfigData = serde_json::from_str(json).unwrap();
        assert_eq!(config.model.as_deref(), Some("gpt-4"));
        assert_eq!(config.taste_learning, Some(false));
        assert_eq!(config.oauth_enforced, Some(true));
    }

    // === Path traversal tests ===

    #[tokio::test]
    async fn test_auth_manager_special_characters_in_path() {
        let tmp = TempDir::new().unwrap();
        let auth_dir = tmp.path().join(".commandcode");
        std::fs::create_dir_all(&auth_dir).unwrap();
        std::fs::write(auth_dir.join("auth.json"), r#"{"apiKey":"k"}"#).unwrap();

        // Path with special characters should work
        let mgr = AuthManager::new(auth_dir, 30);
        let headers = mgr
            .build_headers("/path/with spaces/and-special@chars")
            .await
            .unwrap();
        assert_eq!(headers.get("x-project-slug").unwrap(), "and-special@chars");
    }

    // === Vault-backed tests ===

    fn vault_account(base: &std::path::Path, keys: &[(&str, &str)]) -> AccountStore {
        use crate::accounts::{Account, AccountVault};
        let store = AccountStore::new(base.join("accounts.json"));
        let mut vault = AccountVault::default();
        for (id, key) in keys {
            let acct = Account {
                api_key: SensitiveString::new(*key),
                user_id: id.to_string(),
                user_name: format!("user-{id}"),
                key_name: format!("cli-{id}"),
                authenticated_at: "2026-08-01T00:00:00Z".to_string(),
                label: format!("label-{id}"),
            };
            vault.add(acct).unwrap();
        }
        store.save(&vault).unwrap();
        store
    }

    fn api_key_of(method: &AuthMethod) -> &str {
        match method {
            AuthMethod::ApiKey(k) => k.as_str(),
            AuthMethod::OAuth { token, .. } => token.as_str(),
        }
    }

    #[tokio::test]
    async fn test_auth_manager_vault_active_credential() {
        let tmp = TempDir::new().unwrap();
        let auth_dir = tmp.path().join(".commandcode");
        std::fs::create_dir_all(&auth_dir).unwrap();
        let store = vault_account(tmp.path(), &[("alice", "user_alice_key")]);
        let mgr = AuthManager::with_vault(auth_dir.clone(), 30, store);

        let method = mgr.get_auth_method().await.unwrap();
        assert_eq!(api_key_of(&method), "user_alice_key");
        assert!(mgr.has_vault());
    }

    #[tokio::test]
    async fn test_auth_manager_vault_falls_back_to_auth_file() {
        use crate::accounts::AccountVault;
        let tmp = TempDir::new().unwrap();
        let auth_dir = tmp.path().join(".commandcode");
        std::fs::create_dir_all(&auth_dir).unwrap();
        std::fs::write(auth_dir.join("auth.json"), r#"{"apiKey":"legacy-key"}"#).unwrap();
        let store = AccountStore::new(tmp.path().join("accounts.json"));
        let _ = store.save(&AccountVault::default());
        let mgr = AuthManager::with_vault(auth_dir.clone(), 30, store);

        let method = mgr.get_auth_method().await.unwrap();
        assert_eq!(api_key_of(&method), "legacy-key");
    }

    #[tokio::test]
    async fn test_auth_manager_rotate_to_next() {
        let tmp = TempDir::new().unwrap();
        let auth_dir = tmp.path().join(".commandcode");
        std::fs::create_dir_all(&auth_dir).unwrap();
        let store = vault_account(
            tmp.path(),
            &[
                ("alice", "key_alice"),
                ("bob", "key_bob"),
                ("carol", "key_carol"),
            ],
        );
        let mgr = AuthManager::with_vault(auth_dir.clone(), 30, store);

        assert_eq!(
            api_key_of(&mgr.get_auth_method().await.unwrap()),
            "key_alice"
        );
        let next = mgr.rotate_to_next().await.unwrap();
        assert!(next.is_some(), "rotation should activate bob");
        assert_eq!(api_key_of(&mgr.get_auth_method().await.unwrap()), "key_bob");
    }

    #[tokio::test]
    async fn test_auth_manager_auto_rotate_flag() {
        let tmp = TempDir::new().unwrap();
        let auth_dir = tmp.path().join(".commandcode");
        std::fs::create_dir_all(&auth_dir).unwrap();
        let store = vault_account(tmp.path(), &[("alice", "key_alice")]);
        let mgr = AuthManager::with_vault(auth_dir.clone(), 30, store);

        assert!(!mgr.auto_rotate_enabled().await);
        mgr.set_auto_rotate(true).await.unwrap();
        assert!(mgr.auto_rotate_enabled().await);
    }

    #[tokio::test]
    async fn test_auth_manager_no_vault_rotate_none() {
        let tmp = TempDir::new().unwrap();
        let auth_dir = tmp.path().join(".commandcode");
        std::fs::create_dir_all(&auth_dir).unwrap();
        std::fs::write(auth_dir.join("auth.json"), r#"{"apiKey":"k"}"#).unwrap();
        let mgr = AuthManager::new(auth_dir, 30);
        assert!(mgr.rotate_to_next().await.unwrap().is_none());
        assert!(!mgr.auto_rotate_enabled().await);
    }
}
