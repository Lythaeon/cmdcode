#![no_main]

use libfuzzer_sys::fuzz_target;
use proxy_core::wire_format;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        if let Ok(messages) = serde_json::from_str::<Vec<wire_format::OpenAiMessage>>(text) {
            let _ = wire_format::wire_messages(&messages);
        }
    }
});
