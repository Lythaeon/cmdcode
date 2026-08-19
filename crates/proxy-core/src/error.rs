use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProxyError {
    #[error("auth: {0}")]
    Auth(#[from] AuthError),

    #[error("upstream: {0}")]
    Upstream(#[from] UpstreamError),

    #[error("serialization: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("config: {0}")]
    Config(#[from] ConfigError),

    #[error("model not allowed: {0}")]
    ModelNotAllowed(String),

    #[error("invalid reasoning effort: {0}")]
    InvalidEffort(String),
}

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("auth directory not found: {path}")]
    DirNotFound { path: String },

    #[error("auth file not found: {path}")]
    FileNotFound { path: String },

    #[error("auth file invalid JSON: {path}: {source}")]
    InvalidJson {
        path: String,
        source: serde_json::Error,
    },

    #[error("auth file missing required field '{field}'")]
    MissingField { field: &'static str },

    #[error("no authentication configured (need apiKey or oauthToken)")]
    NoAuthConfigured,

    #[error("token expired")]
    TokenExpired,

    #[error("token refresh failed: {0}")]
    TokenRefreshFailed(String),
}

#[derive(Debug, Error)]
pub enum UpstreamError {
    #[error("connection refused: {host}:{port}")]
    ConnectionRefused { host: String, port: u16 },

    #[error("connection reset")]
    ConnectionReset,

    #[error("timeout after {timeout_secs}s")]
    Timeout { timeout_secs: u64 },

    #[error("TLS error: {0}")]
    Tls(String),

    #[error("upstream returned HTTP {status}: {body}")]
    HttpError { status: u16, body: String },

    #[error("upstream returned non-JSON error: {body}")]
    NonJsonError { body: String },

    #[error("stream closed prematurely")]
    StreamClosedPrematurely,

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("invalid listen address: {0}")]
    InvalidListenAddress(String),

    #[error("invalid upstream URL: {0}")]
    InvalidUpstreamUrl(String),

    #[error("invalid timeout value: {0}")]
    InvalidTimeout(String),

    #[error("model allowlist parse error: {0}")]
    ModelAllowlistParse(String),
}
