//! Core types, configuration, and wire-format translation for the Command Code
//! OpenAI-compatible proxy.

/// Authentication and credential management.
pub mod auth;
/// Proxy configuration loaded from environment variables.
pub mod config;
/// Error types returned by core components.
pub mod error;
/// Model catalog parsed from CLI-bundled model metadata.
pub mod model_catalog;
/// Rate limiting for API requests.
pub mod rate_limiter;
/// Harness detection and configuration.
pub mod setup;
/// Shared newtypes: model identifiers, effort levels, session IDs, etc.
pub mod types;
/// OpenAI and Command Code wire-format types and translation functions.
pub mod wire_format;
