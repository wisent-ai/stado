//! `stado-migrate` — move the active Stado coordinator between registry hosts.
//!
//! Coordinator migration is a first-class Stado operation: the canonical
//! registry decides which coordinator entry is active, and this binary
//! performs the ordered cut-over — preflight the target host, stop the old
//! daemon, flip `active` in the registry through the validated
//! compare-and-swap write path, bootstrap the new daemon through Stado's own
//! deploy machinery, then verify. Remote access goes only through Stado's
//! deploy runner, never through ad-hoc operator commands.

mod plan;
mod run;

use clap::{Parser, Subcommand};
use std::process::ExitCode;

/// Move the active Stado coordinator to another registry coordinator entry.
#[derive(Parser)]
#[command(name = "stado-migrate", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Migrate the active coordinator to the named registry entry.
    Coordinator {
        /// Target coordinator entry (runtime=daemon with a remote host).
        #[arg(long)]
        to: String,
        /// Source entry (default: the one with active=true).
        #[arg(long)]
        from: Option<String>,
        /// Print the ordered plan; change nothing.
        #[arg(long)]
        dry_run: bool,
        /// Copy the device-local store to the target host before the flip.
        #[arg(long)]
        move_local_storage: bool,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Commands::Coordinator {
            to,
            from,
            dry_run,
            move_local_storage,
        } => run::migrate_coordinator(&to, from.as_deref(), dry_run, move_local_storage).await,
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("stado-migrate: {message}");
            ExitCode::FAILURE
        }
    }
}
