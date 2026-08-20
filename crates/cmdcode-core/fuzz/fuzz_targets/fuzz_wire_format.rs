#![no_main]

use libfuzzer_sys::fuzz_target;
use cmdcode_core::wire_format;

fuzz_target!(|data: &[u8]| {
    // Fuzz message wire format conversion
    if let Ok(messages) = serde_json::from_slice::<Vec<wire_format::OpenAiMessage>>(data) {
        let _ = wire_format::wire_messages(&messages);
    }

    // Fuzz tool wire format conversion
    if let Ok(tools) = serde_json::from_slice::<Vec<wire_format::OpenAiTool>>(data) {
        let _ = wire_format::wire_tools(&tools);
    }

    // Fuzz upstream event parsing
    if let Ok(text) = std::str::from_utf8(data) {
        for line in text.lines() {
            let _ = serde_json::from_str::<wire_format::UpstreamEvent>(line);
        }
    }

    // Fuzz chat completion request parsing
    if let Ok(_req) = serde_json::from_slice::<wire_format::ChatCompletionRequest>(data) {
        // Valid request parsed
    }

    // Fuzz model and effort parsing
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = cmdcode_core::types::parse_model_and_effort(text);
    }
});
