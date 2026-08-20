#![no_main]

use libfuzzer_sys::fuzz_target;
use cmdcode_core::types;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = types::parse_model_and_effort(text);
    }
});
