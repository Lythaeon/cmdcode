//! Dynamic client-fingerprint detection for upstream adapters.
//!
//! Each native adapter mimics the *official* client for its API so the
//! upstream sees the same headers it would from a first-party CLI:
//!
//! | Adapter  | Client           | User-Agent                                    | Extra headers |
//! |----------|------------------|-----------------------------------------------|---------------|
//! | gemini   | Gemini CLI       | `GeminiCLI/{version}` (npm/node/platform)     | — |
//! | anthropic| Claude Code      | `claude-cli/{version} (external, cli)`        | `x-app: cli` |
//! | openai   | Codex CLI        | `codex_cli_rs/{version} ({os} {arch}) {term}` | `originator: codex_cli_rs`, `session_id` |
//!
//! Versions are detected at most once per process by running the installed
//! binary (`claude --version`, `codex --version`, `gemini --version`).
//! When a CLI is absent, a well-known fallback version is used so requests
//! still look current.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Run a binary, capture stdout, return trimmed output (None on any error).
fn probe(args: &[&str]) -> Option<String> {
    let (bin, rest) = args.split_first()?;
    let output = std::process::Command::new(bin).args(rest).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Some(stdout.trim().to_string())
}

/// Extract the first `x.y[.z]` version-looking token from text.
fn first_version(text: &str) -> Option<String> {
    for token in text.split_whitespace() {
        let t = token.trim_start_matches('v');
        let has_digit = t.chars().any(|c| c.is_ascii_digit());
        let plausible = t
            .chars()
            .all(|c| c.is_ascii_digit() || c == '.' || c == '-')
            && has_digit;
        if plausible && t.contains('.') {
            return Some(t.trim_end_matches('.').to_string());
        }
    }
    None
}

/// Cached, once-per-process CLI version detection (probe runs at most once
/// per key; the subprocess is ~600ms for Node CLIs so never call hot).
fn cached_version(key: &'static str, probe_args: &[&str], fallback: &'static str) -> String {
    static CACHE: OnceLock<Mutex<HashMap<&'static str, String>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = cache.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(v) = map.get(key) {
        return v.clone();
    }
    let detected = probe(probe_args)
        .as_deref()
        .and_then(first_version)
        .unwrap_or_else(|| fallback.to_string());
    map.insert(key, detected.clone());
    detected
}

/// Claude Code version (`claude --version` → "2.1.215 (Claude Code)").
pub fn claude_cli_version() -> String {
    cached_version("claude", &["claude", "--version"], "2.1.215")
}

/// Codex CLI version (`codex --version` → "codex-cli 0.144.6").
pub fn codex_cli_version() -> String {
    cached_version(
        "codex",
        &["codex", "--version"],
        "0.144.6",
    )
}

/// Gemini CLI version (`gemini --version` → "0.46.0").
pub fn gemini_cli_version() -> String {
    cached_version("gemini", &["gemini", "--version"], "0.46.0")
}

/// Claude Code user-agent:
/// `claude-cli/{version} (external, cli)`
pub fn claude_user_agent() -> String {
    format!(
        "claude-cli/{} (external, cli)",
        claude_cli_version()
    )
}

/// Codex user-agent:
/// `codex_cli_rs/{version} ({os} {arch}) {terminal}`
pub fn codex_user_agent() -> String {
    format!(
        "codex_cli_rs/{} ({os} {arch}) unknown",
        codex_cli_version(),
        os = std::env::consts::OS,
        arch = std::env::consts::ARCH,
    )
}

/// Gemini CLI user-agent: `GeminiCLI/{version}` (the npm/node suffix the CLI
/// appends is platform metadata the API does not validate).
pub fn gemini_user_agent() -> String {
    format!("GeminiCLI/{}", gemini_cli_version())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_first_version_extraction() {
        assert_eq!(
            first_version("2.1.215 (Claude Code)").as_deref(),
            Some("2.1.215")
        );
        assert_eq!(first_version("codex-cli 0.144.6").as_deref(), Some("0.144.6"));
        assert_eq!(first_version("0.46.0").as_deref(), Some("0.46.0"));
        assert_eq!(first_version("no version here"), None);
    }

    #[test]
    fn test_user_agent_shapes() {
        let ua = claude_user_agent();
        assert!(ua.starts_with("claude-cli/"), "{ua}");
        assert!(ua.contains("(external, cli)"), "{ua}");

        let ua = codex_user_agent();
        assert!(ua.starts_with("codex_cli_rs/"), "{ua}");
        assert!(ua.contains("(linux") || ua.contains("(macos"), "{ua}");

        let ua = gemini_user_agent();
        assert!(ua.starts_with("GeminiCLI/"), "{ua}");
    }
}
