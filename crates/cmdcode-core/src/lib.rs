//! Core types, configuration, and wire-format translation for the Command Code
//! OpenAI-compatible proxy.

/// Multi-account credential vault.
/// anthropic wire.
pub mod anthropic_wire;

/// accounts.
pub mod accounts;
/// Authentication and credential management.
/// auth.
pub mod auth;
/// Proxy configuration loaded from environment variables.
/// config.
pub mod config;
/// Error types returned by core components.
/// error.
pub mod error;
/// Model catalog parsed from CLI-bundled model metadata.
/// model catalog.
pub mod model_catalog;

/// Declarative upstream-provider configuration (opencode-style providers map).
/// provider config.
pub mod provider_config;
/// Rate limiting for API requests.
/// rate limiter.
pub mod rate_limiter;
/// Harness detection and configuration.
/// setup.
pub mod setup;
/// Shared newtypes: model identifiers, effort levels, session IDs, etc.
/// types.
pub mod types;
/// OpenAI and Command Code wire-format types and translation functions.
/// wire format.
pub mod wire_format;
