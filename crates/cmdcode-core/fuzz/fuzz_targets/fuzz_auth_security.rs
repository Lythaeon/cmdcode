#![no_main]

use libfuzzer_sys::fuzz_target;
use cmdcode_core::auth;

fuzz_target!(|data: &[u8]| {
    // Fuzz AuthData deserialization
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = serde_json::from_str::<auth::AuthData>(text);
    }

    // Fuzz ConfigData deserialization
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = serde_json::from_str::<auth::ConfigData>(text);
    }

    // Fuzz AuthMethod parsing from raw JSON
    if let Ok(text) = std::str::from_utf8(data) {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(text) {
            // Try to extract auth method from arbitrary JSON
            let _ = val.get("apiKey").and_then(|v| v.as_str());
            let _ = val.get("oauthToken").and_then(|v| v.as_str());
            let _ = val.get("oauthProvider").and_then(|v| v.as_str());
        }
    }
});
