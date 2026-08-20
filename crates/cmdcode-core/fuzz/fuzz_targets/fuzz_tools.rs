#![no_main]

use libfuzzer_sys::fuzz_target;
use cmdcode_core::wire_format;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        if let Ok(tools) = serde_json::from_str::<Vec<wire_format::OpenAiTool>>(text) {
            let _ = wire_format::wire_tools(&tools);
        }
    }
});
