use crate::error::AuthError;
use crate::types::SessionId;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Raw auth file contents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthData {
    /// API key for upstream authentication.
    #[serde(default, alias = "apiKey")]
    pub api_key: Option<String>,
    /// OAuth token for upstream authentication.
    #[serde(default, alias = "oauthToken")]
    pub oauth_token: Option<String>,
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
    ApiKey(String),
    /// OAuth token authentication.
    OAuth {
        /// OAuth access token.
        token: String,
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
    state: Arc<RwLock<Option<CachedAuth>>>,
}

impl AuthManager {
    /// Create a new auth manager that reads credentials from `auth_dir`.
    pub fn new(auth_dir: PathBuf, cache_ttl_secs: u64) -> Self {
        Self {
            auth_dir,
            cache_ttl: Duration::from_secs(cache_ttl_secs),
            state: Arc::new(RwLock::new(None)),
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

        let config_file = self.auth_dir.join("config.json");
        let config = if config_file.exists() {
            tokio::fs::read_to_string(&config_file)
                .await
                .ok()
                .and_then(|c| serde_json::from_str(&c).ok())
                .unwrap_or_default()
        } else {
            ConfigData::default()
        };

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
        headers.insert("x-command-code-version".into(), "1.0.0".into());
        // F-8: Sanitize env var value to prevent header injection via CRLF.
        let cli_env = std::env::var("COMMAND_CODE_ENV")
            .unwrap_or_else(|_| "production".into())
            .chars()
            .filter(|c| *c != '\r' && *c != '\n')
            .collect::<String>();
        headers.insert("x-cli-environment".into(), cli_env);
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
                headers.insert("Authorization".into(), format!("Bearer {}", key));
            }
            AuthMethod::OAuth { token, provider } => {
                headers.insert("x-oauth-token".into(), format!("Bearer {}", token));
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
            AuthMethod::ApiKey(k) => assert_eq!(k, "test-key-12345678"),
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
            AuthMethod::ApiKey(k) => assert_eq!(k, "key1"), // cached
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
                assert_eq!(token, "tok123");
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
                assert_eq!(token, "tok456");
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
            AuthMethod::ApiKey(k) => assert_eq!(k, "key2"), // should see new key
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
}
