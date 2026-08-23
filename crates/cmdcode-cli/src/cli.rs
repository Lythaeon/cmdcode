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

    /// Manage accounts and authentication
    #[command(
        about = "Manage Command Code accounts (list, use, logout, add)",
        long_about = "Manage Command Code accounts stored in ~/.cmdcode/accounts.json.\n\nWithout subcommand: opens the interactive TUI.\nWith subcommand: performs the action directly."
    )]
    Auth {
        #[command(subcommand)]
        command: Option<AuthCommand>,
    },

    /// Send a test request to verify proxy functionality
    #[command(
        about = "Send a test request to verify proxy functionality",
        long_about = "Start the proxy temporarily, send a minimal chat completion request,\nand verify the response. Checks: auth validity, upstream connectivity,\nand wire format translation.\n\nExits with code 0 on success, 1 on failure.\n\nRequires the proxy to NOT be already running on the configured port."
    )]
    Test,

    /// Configure client harnesses to use the proxy
    #[command(
        about = "Configure client harnesses to use the proxy",
        long_about = "Detect installed harnesses (OpenCode, Codex, Hermes, LiteLLM, Ollama, vLLM, Open WebUI)\nand configure them to use cmdcode as the proxy.\n\nRequires an explicit subcommand: 'all' to configure all detected harnesses,\nor a specific harness name.\n\nUse --dry-run to preview changes without writing files.",
        alias = "configure"
    )]
    Setup {
        #[command(subcommand)]
        command: SetupCommand,
    },
}

#[derive(Subcommand)]
pub enum SetupCommand {
    /// Configure all detected harnesses
    #[command(
        about = "Configure all detected harnesses",
        long_about = "Detect all installed harnesses and configure them to use cmdcode as the proxy.\n\nUse --dry-run to preview changes without writing files."
    )]
    All {
        /// Preview configuration without writing files
        #[arg(long)]
        dry_run: bool,

        /// Overwrite existing configuration files
        #[arg(long)]
        force: bool,
    },

    /// Configure OpenCode
    #[command(
        about = "Configure OpenCode to use the proxy",
        long_about = "Configure OpenCode to use cmdcode as the proxy.\nWrites configuration to ~/.config/opencode/opencode.json."
    )]
    OpenCode {
        #[arg(long)]
        dry_run: bool,

        #[arg(long)]
        force: bool,
    },

    /// Configure Codex CLI
    #[command(
        about = "Configure Codex CLI to use the proxy",
        long_about = "Configure Codex CLI to use cmdcode as the proxy.\nWrites configuration to ~/.codex/cmdcode-proxy.toml."
    )]
    Codex {
        #[arg(long)]
        dry_run: bool,

        #[arg(long)]
        force: bool,
    },

    /// Configure Hermes
    #[command(
        about = "Configure Hermes to use the proxy",
        long_about = "Configure Hermes to use cmdcode as the proxy.\nWrites configuration to ~/.hermes/cmdcode-proxy.yaml."
    )]
    Hermes {
        #[arg(long)]
        dry_run: bool,

        #[arg(long)]
        force: bool,
    },

    /// Configure LiteLLM
    #[command(
        about = "Configure LiteLLM to use the proxy",
        long_about = "Configure LiteLLM to use cmdcode as the proxy.\nWrites configuration to litellm_config.json in the current directory."
    )]
    Litellm {
        #[arg(long)]
        dry_run: bool,

        #[arg(long)]
        force: bool,
    },

    /// Configure Ollama
    #[command(
        about = "Configure Ollama to use the proxy",
        long_about = "Configure Ollama to use cmdcode as the proxy.\nWrites configuration to ~/.ollama/cmdcode-proxy.env."
    )]
    Ollama {
        #[arg(long)]
        dry_run: bool,

        #[arg(long)]
        force: bool,
    },

    /// Configure vLLM
    #[command(
        about = "Configure vLLM to use the proxy",
        long_about = "Configure vLLM to use cmdcode as the proxy.\nWrites configuration to cmdcode-proxy.env in the current directory."
    )]
    Vllm {
        #[arg(long)]
        dry_run: bool,

        #[arg(long)]
        force: bool,
    },

    /// Configure Open WebUI
    #[command(
        about = "Configure Open WebUI to use the proxy",
        long_about = "Configure Open WebUI to use cmdcode as the proxy.\nWrites configuration to ~/.open-webui/cmdcode-config.json."
    )]
    OpenWebui {
        #[arg(long)]
        dry_run: bool,

        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand)]
pub enum AuthCommand {
    /// List all stored accounts
    #[command(about = "List stored accounts and show which is active")]
    List,

    /// Switch the active account
    #[command(about = "Switch which account the proxy uses")]
    Use,

    /// Log out one or more accounts
    #[command(about = "Remove one or more accounts")]
    Logout,

    /// Sign in a new account (Studio callback or paste API key)
    #[command(about = "Sign in a new Command Code account")]
    Add,

    /// Toggle auto-rotate setting
    #[command(
        about = "Toggle auto-rotate (switch accounts on credit/rate-limit errors)",
        long_about = "Toggle the auto-rotate setting. When enabled, the proxy automatically\nswitches to the next account when the active one is rejected or rate-limited."
    )]
    AutoRotate {
        /// Set auto-rotate to on or off
        #[arg(value_name = "on|off")]
        state: Option<String>,
    },
}
