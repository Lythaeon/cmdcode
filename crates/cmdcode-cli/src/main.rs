//! Command Code OpenAI-compatible proxy server binary.

mod cli;
mod commands;

use clap::Parser;
use cli::{Cli, Commands, SetupCommand};

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Serve => {
            if let Err(e) = commands::serve::run() {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Commands::Status => commands::status::run(),
        Commands::Models => commands::models::run(),
        Commands::Config => commands::config::run(),
        Commands::Auth => commands::auth::run(),
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
