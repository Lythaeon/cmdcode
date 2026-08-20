#![no_main]

use libfuzzer_sys::fuzz_target;
use cmdcode_core::error;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        // Fuzz all error type Display implementations
        let err = error::ConfigError::InvalidListenAddress(text.to_string());
        let _ = err.to_string();

        let err = error::ConfigError::InvalidUpstreamUrl(text.to_string());
        let _ = err.to_string();

        let err = error::ConfigError::InvalidTimeout(text.to_string());
        let _ = err.to_string();

        let err = error::ConfigError::ModelAllowlistParse(text.to_string());
        let _ = err.to_string();

        let err = error::AuthError::FileNotFound {
            path: text.to_string(),
        };
        let _ = err.to_string();

        let err = error::AuthError::TokenRefreshFailed(text.to_string());
        let _ = err.to_string();

        let err = error::UpstreamError::ConnectionRefused {
            host: text.to_string(),
            port: 80,
        };
        let _ = err.to_string();

        let err = error::UpstreamError::Tls(text.to_string());
        let _ = err.to_string();

        let err = error::UpstreamError::HttpError {
            status: 500,
            body: text.to_string(),
        };
        let _ = err.to_string();

        let err = error::UpstreamError::NonJsonError {
            body: text.to_string(),
        };
        let _ = err.to_string();

        let err = error::ProxyError::ModelNotAllowed(text.to_string());
        let _ = err.to_string();

        let err = error::ProxyError::InvalidEffort(text.to_string());
        let _ = err.to_string();
    }
});
