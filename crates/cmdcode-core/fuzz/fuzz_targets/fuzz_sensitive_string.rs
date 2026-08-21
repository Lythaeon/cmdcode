#![no_main]

use libfuzzer_sys::fuzz_target;
use cmdcode_core::types::SensitiveString;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        // Fuzz SensitiveString creation and operations
        let s = SensitiveString::new(text.to_string());
        
        // Test basic operations
        let _ = s.as_str();
        let _ = s.is_empty();
        let _ = s.len();
        
        // Test Display (should never reveal content)
        let display = format!("{}", s);
        assert!(display == "[REDACTED]", "Display should never reveal content");
        
        // Test equality
        let s2 = SensitiveString::new(text.to_string());
        let _ = s == s2;
        let _ = s != s2;
        
        // Test hash
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        s.hash(&mut hasher);
        let _ = hasher.finish();
        
        // Test Deref
        let _: &str = &s;
        
        // Test Serialize/Deserialize
        if let Ok(json) = serde_json::to_string(&s) {
            let _: Result<SensitiveString, _> = serde_json::from_str(&json);
        }
        
        // Test ZeroizeOnDrop (just drop it - should not panic)
        drop(s);
    }
});
