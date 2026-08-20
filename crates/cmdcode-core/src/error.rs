use thiserror::Error;

/// Top-level error type for the proxy.
#[derive(Debug, Error)]
pub enum ProxyError {
    /// Authentication or credential error.
    #[error("auth: {0}")]
    Auth(#[from] AuthError),

    /// Upstream API error.
    #[error("upstream: {0}")]
    Upstream(#[from] UpstreamError),

    /// JSON serialization / deserialization error.
    #[error("serialization: {0}")]
    Serialization(#[from] serde_json::Error),

    /// Configuration parsing error.
    #[error("config: {0}")]
    Config(#[from] ConfigError),

    /// Requested model is not in the allowlist.
    #[error("model not allowed: {0}")]
    ModelNotAllowed(String),

    /// Invalid reasoning effort level.
    #[error("invalid reasoning effort: {0}")]
    InvalidEffort(String),
}

/// Errors related to loading or using authentication credentials.
#[derive(Debug, Error)]
pub enum AuthError {
    /// The configured auth directory does not exist.
    #[error("auth directory not found: {path}")]
    DirNotFound {
        /// Path to the missing directory.
        path: String,
    },

    /// An auth or config file does not exist.
    #[error("auth file not found: {path}")]
    FileNotFound {
        /// Path to the missing file.
        path: String,
    },

    /// An auth file contains invalid JSON.
    #[error("auth file invalid JSON: {path}: {source}")]
    InvalidJson {
        /// Path to the file with invalid JSON.
        path: String,
        /// The underlying parse error.
        source: serde_json::Error,
    },

    /// A required field is missing from an auth file.
    #[error("auth file missing required field '{field}'")]
    MissingField {
        /// Name of the missing field.
        field: &'static str,
    },

    /// No authentication credential is configured.
    #[error("no authentication configured (need apiKey or oauthToken)")]
    NoAuthConfigured,

    /// The cached token has expired.
    #[error("token expired")]
    TokenExpired,

    /// Refreshing the token from disk or upstream failed.
    #[error("token refresh failed: {0}")]
    TokenRefreshFailed(String),
}

/// Errors communicating with the upstream Command Code API.
#[derive(Debug, Error)]
pub enum UpstreamError {
    /// TCP connection was refused.
    #[error("connection refused: {host}:{port}")]
    ConnectionRefused {
        /// Host that refused the connection.
        host: String,
        /// Port that refused the connection.
        port: u16,
    },

    /// Connection was reset by the remote host.
    #[error("connection reset")]
    ConnectionReset,

    /// Request timed out after `timeout_secs` seconds.
    #[error("timeout after {timeout_secs}s")]
    Timeout {
        /// Timeout duration in seconds.
        timeout_secs: u64,
    },

    /// TLS handshake or protocol error.
    #[error("TLS error: {0}")]
    Tls(String),

    /// Upstream returned a non-200 HTTP status.
    #[error("upstream returned HTTP {status}: {body}")]
    HttpError {
        /// HTTP status code.
        status: u16,
        /// Response body.
        body: String,
    },

    /// Upstream returned a response that is not valid JSON.
    #[error("upstream returned non-JSON error: {body}")]
    NonJsonError {
        /// Raw response body.
        body: String,
    },

    /// The SSE stream was closed before a finish event.
    #[error("stream closed prematurely")]
    StreamClosedPrematurely,

    /// Standard I/O error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Errors parsing proxy configuration from environment variables.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// The listen address format is invalid.
    #[error("invalid listen address: {0}")]
    InvalidListenAddress(String),

    /// The upstream URL format is invalid.
    #[error("invalid upstream URL: {0}")]
    InvalidUpstreamUrl(String),

    /// A timeout or numeric configuration value is invalid.
    #[error("invalid timeout value: {0}")]
    InvalidTimeout(String),

    /// The model allowlist could not be parsed.
    #[error("model allowlist parse error: {0}")]
    ModelAllowlistParse(String),
}
