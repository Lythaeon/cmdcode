//! Command Code OpenAI-compatible proxy server binary.

mod cli;
mod commands;

use clap::Parser;
use cli::{Cli, Commands};

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
    }
}
