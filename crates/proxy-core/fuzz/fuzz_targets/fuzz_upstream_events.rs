#![no_main]

use libfuzzer_sys::fuzz_target;
use proxy_core::wire_format;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        for line in text.lines() {
            let _ = serde_json::from_str::<wire_format::UpstreamEvent>(line);
        }
    }
});
