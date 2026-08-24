//! Server-side response-session storage for the OpenAI Responses API.
//!
//! Stores each completed response's full conversation (in internal OpenAI
//! message form) keyed by its `resp_*` id so subsequent requests can chain
//! via `previous_response_id` without resending history. Entries expire on
//! a TTL and the map is capped to bound memory.

use cmdcode_core::wire_format::OpenAiMessage;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// TTL for stored responses.
const ENTRY_TTL: Duration = Duration::from_secs(3600);
/// Maximum number of retained responses; oldest are evicted first.
const MAX_ENTRIES: usize = 10_000;

struct Entry {
    messages: Vec<OpenAiMessage>,
    created: Instant,
}

/// Shared store behind a mutex; cheap to clone via Arc at construction.
#[derive(Default)]
pub struct ResponseSessionStore {
    entries: Mutex<HashMap<String, Entry>>,
}

impl ResponseSessionStore {
    /// Store the conversation under `id`, evicting stale/expired entries.
    pub fn insert(&self, id: String, messages: Vec<OpenAiMessage>) {
        let mut entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        let now = Instant::now();

        // Lazy eviction: drop expired entries and enforce the cap.
        if entries.len() >= MAX_ENTRIES {
            entries.retain(|_, e| now.duration_since(e.created) < ENTRY_TTL);
            while entries.len() >= MAX_ENTRIES {
                // Remove an arbitrary oldest entry.
                let Some(oldest) = entries
                    .iter()
                    .min_by_key(|(_, e)| e.created)
                    .map(|(k, _)| k.clone())
                else {
                    break;
                };
                entries.remove(&oldest);
            }
        }

        entries.insert(
            id,
            Entry {
                messages,
                created: now,
            },
        );
    }

    /// Fetch the stored conversation, returning a copy.
    pub fn get(&self, id: &str) -> Option<Vec<OpenAiMessage>> {
        let entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        let entry = entries.get(id)?;
        if Instant::now().duration_since(entry.created) >= ENTRY_TTL {
            return None;
        }
        Some(entry.messages.clone())
    }

    /// Number of live entries (test helper).
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.lock().unwrap_or_else(|p| p.into_inner()).len()
    }

    /// Whether the store is empty (companion to [`Self::len`]).
    #[cfg(test)]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn msg(role: &str, text: &str) -> OpenAiMessage {
        OpenAiMessage {
            role: role.into(),
            content: Some(json!(text)),
            tool_calls: None,
            tool_call_id: None,
        }
    }
    use serde_json::json;

    #[test]
    fn test_roundtrip() {
        let store = ResponseSessionStore::default();
        store.insert("resp_1".into(), vec![msg("system", "s"), msg("user", "hi")]);
        let got = store.get("resp_1").unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[1].role, "user");
    }

    #[test]
    fn test_missing_and_overwrite() {
        let store = ResponseSessionStore::default();
        assert!(store.get("nope").is_none());
        store.insert("resp_2".into(), vec![msg("user", "a")]);
        store.insert("resp_2".into(), vec![msg("user", "b")]);
        assert_eq!(store.get("resp_2").unwrap()[0].content, Some(json!("b")));
    }
}
