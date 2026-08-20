#![no_main]

use libfuzzer_sys::fuzz_target;
use cmdcode_core::model_catalog;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = model_catalog::parse_models_md(text);
    }
});
