pub use crate::types::RateLimitBackend;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Rate limit configuration for a single API key.
#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    /// Maximum number of requests allowed per window.
    pub max_requests: u64,
    /// Time window in seconds.
    pub window_secs: u64,
    /// Backend to use for rate limiting.
    pub backend: RateLimitBackend,
    /// Redis URL (only used if backend is Redis).
    pub redis_url: Option<String>,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            max_requests: 100,
            window_secs: 60,
            backend: RateLimitBackend::Local,
            redis_url: None,
        }
    }
}

/// Rate limit state for a single API key.
#[derive(Debug)]
struct RateLimitEntry {
    /// Number of requests in the current window.
    count: u64,
    /// Start of the current window.
    window_start: Instant,
}

/// In-memory rate limiter that tracks requests per API key.
pub struct RateLimiter {
    /// Configuration.
    config: RateLimitConfig,
    /// Per-key rate limit state.
    state: Arc<RwLock<HashMap<String, RateLimitEntry>>>,
}

impl RateLimiter {
    /// Create a new rate limiter with the given configuration.
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            config,
            state: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Check if a request from the given API key is allowed.
    /// Returns true if allowed, false if rate limited.
    pub async fn check_rate_limit(&self, api_key: &str) -> bool {
        // Skip rate limiting for empty keys (unauthenticated)
        if api_key.is_empty() {
            return true;
        }

        let now = Instant::now();
        let window = Duration::from_secs(self.config.window_secs);

        let mut state = self.state.write().await;

        let entry = state
            .entry(api_key.to_string())
            .or_insert_with(|| RateLimitEntry {
                count: 0,
                window_start: now,
            });

        // Check if window has expired
        if now.duration_since(entry.window_start) > window {
            // Reset the window
            entry.count = 1;
            entry.window_start = now;
            return true;
        }

        // Check if rate limit exceeded
        if entry.count >= self.config.max_requests {
            return false;
        }

        entry.count += 1;
        true
    }

    /// Get the number of remaining requests for an API key.
    pub async fn remaining_requests(&self, api_key: &str) -> u64 {
        let now = Instant::now();
        let window = Duration::from_secs(self.config.window_secs);
        let state = self.state.read().await;

        if let Some(entry) = state.get(api_key) {
            if now.duration_since(entry.window_start) <= window {
                return self.config.max_requests.saturating_sub(entry.count);
            }
        }
        self.config.max_requests
    }

    /// Get the time until the current window resets for an API key.
    pub async fn reset_time(&self, api_key: &str) -> Duration {
        let now = Instant::now();
        let window = Duration::from_secs(self.config.window_secs);
        let state = self.state.read().await;

        if let Some(entry) = state.get(api_key) {
            let elapsed = now.duration_since(entry.window_start);
            if elapsed < window {
                return window - elapsed;
            }
        }
        Duration::ZERO
    }

    /// Clear all rate limit state.
    pub async fn clear(&self) {
        let mut state = self.state.write().await;
        state.clear();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_rate_limiter_allows_within_limit() {
        let limiter = RateLimiter::new(RateLimitConfig {
            max_requests: 5,
            window_secs: 60,
            backend: RateLimitBackend::Local,
            redis_url: None,
        });

        // First 5 requests should be allowed
        for _ in 0..5 {
            assert!(limiter.check_rate_limit("key1").await);
        }

        // 6th request should be rate limited
        assert!(!limiter.check_rate_limit("key1").await);
    }

    #[tokio::test]
    async fn test_rate_limiter_different_keys() {
        let limiter = RateLimiter::new(RateLimitConfig {
            max_requests: 2,
            window_secs: 60,
            backend: RateLimitBackend::Local,
            redis_url: None,
        });

        // Different keys should have separate limits
        assert!(limiter.check_rate_limit("key1").await);
        assert!(limiter.check_rate_limit("key2").await);
        assert!(limiter.check_rate_limit("key1").await);
        assert!(limiter.check_rate_limit("key2").await);

        // Both exhausted
        assert!(!limiter.check_rate_limit("key1").await);
        assert!(!limiter.check_rate_limit("key2").await);

        // New key should work
        assert!(limiter.check_rate_limit("key3").await);
    }

    #[tokio::test]
    async fn test_rate_limiter_empty_key() {
        let limiter = RateLimiter::new(RateLimitConfig {
            max_requests: 1,
            window_secs: 60,
            backend: RateLimitBackend::Local,
            redis_url: None,
        });

        // Empty key should always be allowed (unauthenticated)
        assert!(limiter.check_rate_limit("").await);
        assert!(limiter.check_rate_limit("").await);
    }

    #[tokio::test]
    async fn test_rate_limiter_remaining_requests() {
        let limiter = RateLimiter::new(RateLimitConfig {
            max_requests: 10,
            window_secs: 60,
            backend: RateLimitBackend::Local,
            redis_url: None,
        });

        assert_eq!(limiter.remaining_requests("key1").await, 10);
        limiter.check_rate_limit("key1").await;
        limiter.check_rate_limit("key1").await;
        assert_eq!(limiter.remaining_requests("key1").await, 8);
    }

    #[tokio::test]
    async fn test_rate_limiter_clear() {
        let limiter = RateLimiter::new(RateLimitConfig {
            max_requests: 2,
            window_secs: 60,
            backend: RateLimitBackend::Local,
            redis_url: None,
        });

        // Exhaust the limit
        assert!(limiter.check_rate_limit("key1").await);
        assert!(limiter.check_rate_limit("key1").await);
        assert!(!limiter.check_rate_limit("key1").await);

        // Clear and verify
        limiter.clear().await;
        assert!(limiter.check_rate_limit("key1").await);
    }
}
