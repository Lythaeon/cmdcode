//! Per-provider credential storage for non-Command-Code upstreams.
//!
//! API keys for openai/anthropic/gemini providers live in
//! `~/.cmdcode/secrets.json` (chmod 0600), keyed by the provider id from
//! `providers.json`. Keeping them out of `providers.json` means that file
//! stays shareable/checkpointable while secrets stay protected.
//!
//! Resolution order used by the router (see
//! [`crate::provider_config`]): explicit `options.apiKey` first, then this
//! store, then nothing.

use crate::error::AuthError;
use crate::types::SensitiveString;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Default, Serialize, Deserialize)]
struct SecretFile {
    #[serde(default)]
    keys: BTreeMap<String, SensitiveString>,
}

/// Store for provider API keys.
#[derive(Debug)]
pub struct ProviderSecretStore {
    path: PathBuf,
}

impl Default for ProviderSecretStore {
    fn default() -> Self {
        Self::new(Self::default_path().unwrap_or_else(|| PathBuf::from(".cmdcode-secrets")))
    }
}

impl ProviderSecretStore {
    /// Override path (tests).
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// `~/.cmdcode/secrets.json`, overridable with `CMDCODE_SECRETS_FILE`.
    pub fn default_path() -> Option<PathBuf> {
        if let Ok(p) = std::env::var("CMDCODE_SECRETS_FILE") {
            return Some(PathBuf::from(p));
        }
        dirs::home_dir().map(|h| h.join(".cmdcode").join("secrets.json"))
    }

    fn load(&self) -> Result<SecretFile, AuthError> {
        let content = match std::fs::read_to_string(&self.path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(SecretFile::default()),
            Err(e) => return Err(AuthError::Io(format!("read {}: {e}", self.path.display()))),
        };
        serde_json::from_str(&content)
            .map_err(|e| AuthError::Io(format!("parse {}: {e}", self.path.display())))
    }

    fn save(&self, file: &SecretFile) -> Result<(), AuthError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| AuthError::Io(format!("create {}: {e}", parent.display())))?;
        }
        let content =
            serde_json::to_string_pretty(file).map_err(|e| AuthError::Io(e.to_string()))?;
        std::fs::write(&self.path, content)
            .map_err(|e| AuthError::Io(format!("write {}: {e}", self.path.display())))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(&self.path, perms)
                .map_err(|e| AuthError::Io(format!("chmod {}: {e}", self.path.display())))?;
        }
        Ok(())
    }

    /// Store the key for a provider id.
    pub fn set(&self, provider_id: &str, key: &str) -> Result<(), AuthError> {
        let mut file = self.load()?;
        file.keys
            .insert(provider_id.to_string(), SensitiveString::new(key.trim()));
        self.save(&file)
    }

    /// Fetch the key for a provider id, if stored.
    pub fn get(&self, provider_id: &str) -> Option<String> {
        let file = self.load().ok()?;
        file.keys.get(provider_id).map(|k| k.as_str().to_string())
    }

    /// Remove the key for a provider id. Returns whether one existed.
    pub fn remove(&self, provider_id: &str) -> Result<bool, AuthError> {
        let mut file = self.load()?;
        let existed = file.keys.remove(provider_id).is_some();
        if existed {
            self.save(&file)?;
        }
        Ok(existed)
    }

    /// Ids with stored keys.
    pub fn ids(&self) -> Vec<String> {
        self.load()
            .ok()
            .map(|f| f.keys.keys().cloned().collect())
            .unwrap_or_default()
    }
}

/// Resolve an entry's effective API key: explicit `options.apiKey`
/// (with `{env:VAR}` interpolation) wins, then the secret store.
pub fn resolve_api_key(provider_id: &str, entry_options_key: Option<&str>) -> Option<String> {
    if let Some(k) = entry_options_key {
        return Some(crate::provider_config::interpolate_env_pub(k));
    }
    ProviderSecretStore::default().get(provider_id)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    struct EnvGuard(TempDir);
    fn store() -> (ProviderSecretStore, EnvGuard) {
        let dir = TempDir::new().unwrap();
        let s = ProviderSecretStore::new(dir.path().join("secrets.json"));
        (s, EnvGuard(dir))
    }

    #[test]
    fn test_set_get_remove_roundtrip() {
        let (store, _guard) = store();
        assert!(store.get("openai").is_none());
        store.set("openai", "sk-test").unwrap();
        assert_eq!(store.get("openai").as_deref(), Some("sk-test"));
        assert!(store.remove("openai").unwrap());
        assert!(store.get("openai").is_none());
        assert!(!store.remove("openai").unwrap());
    }

    #[test]
    fn test_trims_whitespace_and_persists_key() {
        let (store, guard) = store();
        store.set("p", "  sk-abc  ").unwrap();
        assert_eq!(store.get("p").as_deref(), Some("sk-abc"));
        // File exists with 0600 perms and holds the working key.
        let raw = std::fs::read_to_string(guard.0.path().join("secrets.json")).unwrap();
        assert!(raw.contains("sk-abc"), "key must persist on disk");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(guard.0.path().join("secrets.json"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "secrets file must be 0600");
        }
    }

    #[test]
    fn test_ids_listing() {
        let (store, _guard) = store();
        store.set("a", "k1").unwrap();
        store.set("b", "k2").unwrap();
        let mut ids = store.ids();
        ids.sort();
        assert_eq!(ids, vec!["a".to_string(), "b".to_string()]);
    }
}
