#![no_main]

use libfuzzer_sys::fuzz_target;
use cmdcode_core::types::CliEnvironment;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        // Fuzz CliEnvironment parsing from arbitrary strings
        let _ = CliEnvironment::from_str_opt(text);
        
        // Test as_str on all variants
        let _ = CliEnvironment::Production.as_str();
        let _ = CliEnvironment::Development.as_str();
        let _ = CliEnvironment::Staging.as_str();
        
        // Test Display trait
        let _ = format!("{}", CliEnvironment::Production);
        let _ = format!("{}", CliEnvironment::Development);
        let _ = format!("{}", CliEnvironment::Staging);
        
        // Test equality
        let _ = CliEnvironment::Production == CliEnvironment::Production;
        let _ = CliEnvironment::Development == CliEnvironment::Development;
        let _ = CliEnvironment::Staging == CliEnvironment::Staging;
        let _ = CliEnvironment::Production == CliEnvironment::Development;
    }
});
