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
    #[serde(default, alias = "apiKey")]
    pub api_key: Option<String>,
    #[serde(default, alias = "oauthToken")]
    pub oauth_token: Option<String>,
    #[serde(default, alias = "oauthProvider")]
    pub oauth_provider: Option<String>,
    #[serde(default, alias = "userId")]
    pub user_id: Option<String>,
    #[serde(default, alias = "userName")]
    pub user_name: Option<String>,
}

/// Raw config file contents.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConfigData {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default, alias = "tasteLearning")]
    pub taste_learning: Option<bool>,
    #[serde(default, alias = "oauthEnforced")]
    pub oauth_enforced: Option<bool>,
}

/// Authentication method.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthMethod {
    ApiKey(String),
    OAuth { token: String, provider: String },
}

impl AuthMethod {
    pub fn is_valid(&self) -> bool {
        match self {
            AuthMethod::ApiKey(key) => key.len() > 10,
            AuthMethod::OAuth { token, .. } => token.len() > 10,
        }
    }
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
    pub fn new(auth_dir: PathBuf, cache_ttl_secs: u64) -> Self {
        Self {
            auth_dir,
            cache_ttl: Duration::from_secs(cache_ttl_secs),
            state: Arc::new(RwLock::new(None)),
        }
    }

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

    pub async fn health_check(&self) -> HashMap<String, serde_json::Value> {
        let mut result = HashMap::new();
        result.insert(
            "auth_dir".into(),
            serde_json::Value::String(self.auth_dir.display().to_string()),
        );

        let auth_file = self.auth_dir.join("auth.json");
        let config_file = self.auth_dir.join("config.json");

        result.insert(
            "auth_file_exists".into(),
            serde_json::Value::Bool(auth_file.exists()),
        );
        result.insert(
            "config_file_exists".into(),
            serde_json::Value::Bool(config_file.exists()),
        );

        match self.get_auth_method().await {
            Ok(AuthMethod::ApiKey(_)) => {
                result.insert("auth_method".into(), "api_key".into());
                result.insert("auth_valid".into(), true.into());
            }
            Ok(AuthMethod::OAuth { .. }) => {
                result.insert("auth_method".into(), "oauth".into());
                result.insert("auth_valid".into(), true.into());
            }
            Err(e) => {
                result.insert("error".into(), e.to_string().into());
            }
        }

        result
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
                .map_err(|_| AuthError::FileNotFound {
                    path: auth_file.display().to_string(),
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
        headers.insert(
            "x-cli-environment".into(),
            std::env::var("COMMAND_CODE_ENV").unwrap_or_else(|_| "production".into()),
        );
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
}
