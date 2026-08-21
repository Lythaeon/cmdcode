#![no_main]

use libfuzzer_sys::fuzz_target;
use cmdcode_core::types::Environment;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        // Fuzz Environment parsing from arbitrary strings
        let _ = Environment::from_str_opt(text);
        
        // Test as_str on all variants
        let _ = Environment::Production.as_str();
        let _ = Environment::Development.as_str();
        let _ = Environment::Staging.as_str();
        
        // Test Display trait
        let _ = format!("{}", Environment::Production);
        let _ = format!("{}", Environment::Development);
        let _ = format!("{}", Environment::Staging);
        
        // Test equality
        let _ = Environment::Production == Environment::Production;
        let _ = Environment::Development == Environment::Development;
        let _ = Environment::Staging == Environment::Staging;
        let _ = Environment::Production == Environment::Development;
    }
});
