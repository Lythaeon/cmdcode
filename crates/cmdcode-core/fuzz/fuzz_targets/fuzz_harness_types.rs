#![no_main]

use libfuzzer_sys::fuzz_target;
use cmdcode_core::setup::HarnessType;

fuzz_target!(|data: &[u8]| {
    // Fuzz harness type parsing from arbitrary strings
    if let Ok(text) = std::str::from_utf8(data) {
        // Test from_str - should never panic
        let _ = HarnessType::from_str(text);

        // Test matches_filter - should never panic on any harness type
        let harness_types = [
            HarnessType::OpenCode,
            HarnessType::Codex,
            HarnessType::Hermes,
            HarnessType::LiteLLM,
            HarnessType::Ollama,
            HarnessType::Vllm,
            HarnessType::OpenWebUI,
        ];

        for h in &harness_types {
            let _ = h.matches_filter(text);
            let _ = h.name();
        }

        // Test Custom variant
        let custom = HarnessType::Custom(text.to_string());
        let _ = custom.matches_filter(text);
        let _ = custom.name();
    }
});
