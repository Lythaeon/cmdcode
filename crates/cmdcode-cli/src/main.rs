//! Command Code OpenAI-compatible proxy server binary.

mod auth;
mod cli;
mod commands;

use clap::Parser;
use cli::{AuthCommand, Cli, Commands, SetupCommand};

fn main() {
    // Initialize tracing for CLI output
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .try_init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Serve => {
            if let Err(e) = commands::serve::run() {
                tracing::error!(error = %e, "failed to start proxy");
                std::process::exit(1);
            }
        }
        Commands::Status => commands::status::run(),
        Commands::Models => commands::models::run(),
        Commands::Config => commands::config::run(),
        Commands::Auth { command } => match command {
            None => auth::run(),
            Some(AuthCommand::List) => auth::list(),
            Some(AuthCommand::Use) => auth::use_account(),
            Some(AuthCommand::Logout) => auth::logout(),
            Some(AuthCommand::Add) => auth::add(),
            Some(AuthCommand::AutoRotate { state }) => auth::toggle_auto_rotate(state.as_deref()),
        },
        Commands::Test => commands::test::run(),
        Commands::Setup { command } => match command {
            SetupCommand::All { dry_run, force } => {
                commands::setup::run_all(dry_run, force);
            }
            SetupCommand::OpenCode { dry_run, force } => {
                commands::setup::run_harness("opencode", dry_run, force);
            }
            SetupCommand::Codex { dry_run, force } => {
                commands::setup::run_harness("codex", dry_run, force);
            }
            SetupCommand::Hermes { dry_run, force } => {
                commands::setup::run_harness("hermes", dry_run, force);
            }
            SetupCommand::Litellm { dry_run, force } => {
                commands::setup::run_harness("litellm", dry_run, force);
            }
            SetupCommand::Ollama { dry_run, force } => {
                commands::setup::run_harness("ollama", dry_run, force);
            }
            SetupCommand::Vllm { dry_run, force } => {
                commands::setup::run_harness("vllm", dry_run, force);
            }
            SetupCommand::OpenWebui { dry_run, force } => {
                commands::setup::run_harness("open-webui", dry_run, force);
            }
        },
    }
}
