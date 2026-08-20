use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "cmdcode",
    version,
    about = "OpenAI-compatible proxy for Command Code API",
    long_about = "Translates OpenAI /v1/chat/completions requests into the Command Code\nupstream wire format, letting you use your Command Code subscription with\nany OpenAI-compatible client (OpenCode, LiteLLM, Python SDK, curl, etc.).\n\nRequires the command-code CLI to be installed and logged in for auth and\nmodel discovery. Configure via COMMAND_CODE_* environment variables."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Start the proxy server
    #[command(
        about = "Start the proxy server",
        long_about = "Start the HTTP proxy on the configured listen address (default 127.0.0.1:18080).\nTranslates OpenAI chat completion requests to the Command Code upstream API,\nsupporting both streaming (SSE) and non-streaming responses.\n\nSet COMMAND_CODE_PROXY_INCOMING_TOKEN to require a bearer token on API routes."
    )]
    Serve,

    /// Check proxy health and auth status
    #[command(
        about = "Check proxy health and auth status",
        long_about = "Verify that the command-code CLI is installed, credentials are present,\nand the model catalog is loaded. This checks local artifacts only — the\nproxy does not need to be running.\n\nExits with code 1 if any check fails."
    )]
    Status,

    /// List available models
    #[command(
        about = "List available models from the catalog",
        long_about = "Print all models discovered from the command-code CLI's bundled models.md.\nShows model ID, name, context window size, and supported reasoning effort\nlevels. Respects COMMAND_CODE_PROXY_MODELS_CATALOG if set."
    )]
    Models,

    /// Show current configuration
    #[command(
        about = "Show current proxy configuration",
        long_about = "Display all configuration values parsed from environment variables.\nShows defaults for unset variables. Useful for debugging configuration\nissues before starting the proxy."
    )]
    Config,

    /// Show authentication status
    #[command(
        about = "Show authentication credential status",
        long_about = "Display the current authentication method (API key or OAuth), credential\npresence, and related metadata. Reads from the command-code CLI's auth\ndirectory (~/.commandcode/ or COMMAND_CODE_AUTH_DIR).\n\nDoes NOT display actual credential values for security."
    )]
    Auth,

    /// Send a test request to verify proxy functionality
    #[command(
        about = "Send a test request to verify proxy functionality",
        long_about = "Start the proxy temporarily, send a minimal chat completion request,\nand verify the response. Checks: auth validity, upstream connectivity,\nand wire format translation.\n\nExits with code 0 on success, 1 on failure.\n\nRequires the proxy to NOT be already running on the configured port."
    )]
    Test,
}
