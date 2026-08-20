#![no_main]

use libfuzzer_sys::fuzz_target;
use cmdcode_core::auth;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = serde_json::from_str::<auth::AuthData>(text);
        let _ = serde_json::from_str::<auth::ConfigData>(text);
    }
});
