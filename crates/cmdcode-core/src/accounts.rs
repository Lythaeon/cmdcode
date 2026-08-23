use crate::error::AuthError;
use crate::types::SensitiveString;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A single Command Code credential (one signed-in account).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    /// API key for the account (`user_*`).
    #[serde(rename = "apiKey")]
    pub api_key: SensitiveString,
    /// User identifier returned by the studio / whoami.
    #[serde(default, alias = "userId")]
    pub user_id: String,
    /// Human-readable user name.
    #[serde(default, alias = "userName")]
    pub user_name: String,
    /// Key name assigned by the studio.
    #[serde(default, alias = "keyName")]
    pub key_name: String,
    /// ISO-8601 timestamp of when the credential was minted.
    #[serde(default, alias = "authenticatedAt")]
    pub authenticated_at: String,
    /// Display name used for this account in the vault (fallback: user_name).
    #[serde(default)]
    pub label: String,
}

impl Account {
    /// Build a maskable display label.
    pub fn display_name(&self) -> &str {
        if !self.label.is_empty() {
            &self.label
        } else if !self.user_name.is_empty() {
            &self.user_name
        } else if !self.user_id.is_empty() {
            &self.user_id
        } else {
            "unnamed"
        }
    }

    /// Stable identifier used to select an account (user_id where available,
    /// otherwise the key name / a hash of the secret).
    pub fn id(&self) -> String {
        if !self.user_id.is_empty() {
            return self.user_id.clone();
        }
        if !self.key_name.is_empty() {
            return self.key_name.clone();
        }
        // The api key itself is sensitive; never use it verbatim as an id.
        // Hash its bytes for a stable, non-reversible identifier.
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.api_key.as_str().hash(&mut h);
        format!("acct-{:016x}", h.finish())
    }
}

/// Vault settings that control proxy behaviour across accounts.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VaultSettings {
    /// When true, the proxy automatically rotates to the next available
    /// account when the active one runs out of credit or is rejected.
    #[serde(default)]
    pub auto_rotate: bool,
    /// Number of accounts (derived at save time; informational only).
    #[serde(default, skip_serializing)]
    pub account_count: usize,
}

/// Persistent multi-account credential store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountVault {
    /// Schema version.
    #[serde(default = "default_version")]
    pub version: u32,
    /// All signed-in accounts.
    pub accounts: Vec<Account>,
    /// Identifier of the account currently in use (None = no active account).
    #[serde(default)]
    pub active: Option<String>,
    /// Vault-wide settings.
    #[serde(default)]
    pub settings: VaultSettings,
}

fn default_version() -> u32 {
    1
}

impl Default for AccountVault {
    fn default() -> Self {
        Self {
            version: 1,
            accounts: Vec::new(),
            active: None,
            settings: VaultSettings::default(),
        }
    }
}

impl AccountVault {
    /// Find the active account.
    pub fn active_account(&self) -> Option<&Account> {
        let active = self.active.as_deref()?;
        self.accounts.iter().find(|a| a.id() == active)
    }

    /// Set the active account by identifier.
    pub fn set_active(&mut self, id: &str) -> Result<(), AuthError> {
        if !self.accounts.iter().any(|a| a.id() == id) {
            return Err(AuthError::AccountNotFound(id.to_string()));
        }
        self.active = Some(id.to_string());
        Ok(())
    }

    /// Add an account, possibly becoming active if it is the first one.
    pub fn add(&mut self, account: Account) -> Result<(), AuthError> {
        let id = account.id();
        if self.accounts.iter().any(|a| a.id() == id) {
            return Err(AuthError::AccountExists(id));
        }
        let first = self.accounts.is_empty() && self.active.is_none();
        self.accounts.push(account);
        if first {
            self.active = Some(id);
        }
        self.settings.account_count = self.accounts.len();
        Ok(())
    }

    /// Remove one or more accounts by id. If the active account is removed,
    /// activate the next available one (if any).
    pub fn remove(&mut self, ids: &[&str]) -> Result<(), AuthError> {
        let removed_active = ids.iter().any(|id| self.active.as_deref() == Some(*id));
        self.accounts.retain(|a| !ids.contains(&a.id().as_str()));
        self.settings.account_count = self.accounts.len();

        if removed_active {
            if let Some(first) = self.accounts.first() {
                self.active = Some(first.id());
            } else {
                self.active = None;
            }
        }
        Ok(())
    }

    /// Rotate `active` to the next account after the current one (wrap-around).
    /// Returns the id of the newly activated account, or `None` when there is
    /// only one (or zero) account.
    pub fn rotate_next(&mut self) -> Option<String> {
        if self.accounts.len() <= 1 {
            return None;
        }
        let idx = self
            .active
            .as_ref()
            .and_then(|active| self.accounts.iter().position(|a| a.id() == *active))
            .unwrap_or(0);
        let next_idx = (idx + 1) % self.accounts.len();
        let next = self.accounts[next_idx].id();
        self.active = Some(next.clone());
        Some(next)
    }

    /// Number of stored accounts.
    pub fn len(&self) -> usize {
        self.accounts.len()
    }

    /// Whether the vault has no accounts.
    pub fn is_empty(&self) -> bool {
        self.accounts.is_empty()
    }
}

/// Persistent store for the account vault backed by a single JSON file.
#[derive(Debug, Clone)]
pub struct AccountStore {
    /// Path to the vault JSON file.
    path: PathBuf,
}

impl AccountStore {
    /// Default vault path: `~/.cmdcode/accounts.json` (override with
    /// `COMMAND_CODE_ACCOUNTS_FILE`).
    pub fn default_path() -> PathBuf {
        if let Ok(p) = std::env::var("COMMAND_CODE_ACCOUNTS_FILE") {
            if !p.trim().is_empty() {
                return PathBuf::from(p);
            }
        }
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".cmdcode")
            .join("accounts.json")
    }

    /// Create a store rooted at `path`.
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Path backing this store.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Load the vault, returning an empty/default vault if missing.
    pub fn load(&self) -> Result<AccountVault, AuthError> {
        if !self.path.exists() {
            return Ok(AccountVault::default());
        }
        let content =
            std::fs::read_to_string(&self.path).map_err(|e| AuthError::Io(e.to_string()))?;
        let vault: AccountVault =
            serde_json::from_str(&content).map_err(|e| AuthError::InvalidJson {
                path: self.path.display().to_string(),
                source: e,
            })?;
        Ok(vault)
    }

    /// Save the vault atomically (write temp + rename) with `0600` perms.
    pub fn save(&self, vault: &AccountVault) -> Result<(), AuthError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| AuthError::Io(e.to_string()))?;
        }
        let content =
            serde_json::to_string_pretty(vault).map_err(|e| AuthError::Io(e.to_string()))?;

        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, content)
            .map_err(|e| AuthError::Io(format!("{}: {e}", tmp.display())))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&tmp, &self.path)
            .map_err(|e| AuthError::Io(format!("rename -> {}: {e}", self.path.display())))?;
        Ok(())
    }
}

impl Default for AccountStore {
    fn default() -> Self {
        Self::new(Self::default_path())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn acct(id: &str, key: &str) -> Account {
        Account {
            api_key: SensitiveString::new(key),
            user_id: id.to_string(),
            user_name: format!("user-{id}"),
            key_name: format!("cli-{id}"),
            authenticated_at: "2026-08-01T00:00:00Z".to_string(),
            label: format!("label-{id}"),
        }
    }

    #[test]
    fn test_vault_add_and_active() {
        let mut v = AccountVault::default();
        v.add(acct("a", "user_aaa")).unwrap();
        assert_eq!(
            v.active.as_deref(),
            Some("a"),
            "first account becomes active"
        );
        assert_eq!(v.len(), 1);
        assert_eq!(v.active_account().unwrap().user_id, "a");
    }

    #[test]
    fn test_vault_duplicate_rejected() {
        let mut v = AccountVault::default();
        v.add(acct("a", "user_aaa")).unwrap();
        let err = v.add(acct("a", "user_bbb")).unwrap_err();
        assert!(matches!(err, AuthError::AccountExists(_)));
    }

    #[test]
    fn test_vault_set_active_unknown() {
        let mut v = AccountVault::default();
        v.add(acct("a", "user_aaa")).unwrap();
        assert!(v.set_active("nope").is_err());
        v.set_active("a").unwrap();
    }

    #[test]
    fn test_vault_remove_active_falls_back() {
        let mut v = AccountVault::default();
        v.add(acct("a", "user_aaa")).unwrap();
        v.add(acct("b", "user_bbb")).unwrap();
        v.remove(&["a"]).unwrap();
        assert_eq!(v.active.as_deref(), Some("b"));
        assert_eq!(v.len(), 1);
    }

    #[test]
    fn test_vault_remove_all() {
        let mut v = AccountVault::default();
        v.add(acct("a", "user_aaa")).unwrap();
        v.remove(&["a"]).unwrap();
        assert!(v.active.is_none());
        assert!(v.is_empty());
    }

    #[test]
    fn test_vault_rotate_wraps() {
        let mut v = AccountVault::default();
        v.add(acct("a", "user_aaa")).unwrap();
        v.add(acct("b", "user_bbb")).unwrap();
        v.add(acct("c", "user_ccc")).unwrap();
        v.set_active("a").unwrap();
        assert_eq!(v.rotate_next().as_deref(), Some("b"));
        assert_eq!(v.rotate_next().as_deref(), Some("c"));
        assert_eq!(v.rotate_next().as_deref(), Some("a"), "wraps around");
    }

    #[test]
    fn test_vault_rotate_single_returns_none() {
        let mut v = AccountVault::default();
        v.add(acct("a", "user_aaa")).unwrap();
        assert!(v.rotate_next().is_none());
    }

    #[test]
    fn test_store_load_missing_returns_default() {
        let tmp = TempDir::new().unwrap();
        let store = AccountStore::new(tmp.path().join("accounts.json"));
        let v = store.load().unwrap();
        assert!(v.is_empty());
        assert!(!v.settings.auto_rotate);
    }

    #[test]
    fn test_store_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("accounts.json");
        let store = AccountStore::new(path.clone());
        let mut v = AccountVault::default();
        v.add(acct("a", "user_aaa")).unwrap();
        v.add(acct("b", "user_bbb")).unwrap();
        v.set_active("b").unwrap();
        v.settings.auto_rotate = true;
        store.save(&v).unwrap();

        let loaded = store.load().unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.active.as_deref(), Some("b"));
        assert!(loaded.settings.auto_rotate);
        // Secrets survive the round-trip.
        let b = loaded.accounts.iter().find(|a| a.user_id == "b").unwrap();
        assert_eq!(b.api_key.as_str(), "user_bbb");
    }

    #[test]
    fn test_store_overwrites() {
        let tmp = TempDir::new().unwrap();
        let store = AccountStore::new(tmp.path().join("accounts.json"));
        let mut v = AccountVault::default();
        v.add(acct("a", "user_aaa")).unwrap();
        store.save(&v).unwrap();
        let mut v2 = AccountVault::default();
        v2.add(acct("c", "user_ccc")).unwrap();
        store.save(&v2).unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded.accounts[0].user_id, "c");
    }
}
