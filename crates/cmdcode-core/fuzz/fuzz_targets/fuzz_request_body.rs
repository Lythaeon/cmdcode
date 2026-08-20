#![no_main]

use libfuzzer_sys::fuzz_target;
use cmdcode_core::wire_format;

fuzz_target!(|data: &[u8]| {
    let _ = serde_json::from_slice::<wire_format::ChatCompletionRequest>(data);
});
