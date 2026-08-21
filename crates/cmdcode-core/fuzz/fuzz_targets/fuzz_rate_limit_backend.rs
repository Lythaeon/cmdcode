#![no_main]

use libfuzzer_sys::fuzz_target;
use cmdcode_core::types::RateLimitBackend;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        // Fuzz RateLimitBackend parsing from arbitrary strings
        let _ = RateLimitBackend::from_str_opt(text);
        
        // Test as_str on all variants
        let _ = RateLimitBackend::Local.as_str();
        let _ = RateLimitBackend::Redis.as_str();
        
        // Test Display trait
        let _ = format!("{}", RateLimitBackend::Local);
        let _ = format!("{}", RateLimitBackend::Redis);
        
        // Test equality
        let _ = RateLimitBackend::Local == RateLimitBackend::Local;
        let _ = RateLimitBackend::Redis == RateLimitBackend::Redis;
        let _ = RateLimitBackend::Local == RateLimitBackend::Redis;
    }
});
