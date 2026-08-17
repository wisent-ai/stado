//! `stado_fleet` — the fleet-management entry point, kept for the hosts and
//! scripts that already invoke it by name.
//!
//! Every command, flag and word of output comes from
//! [`stado::cli::fleet`], which is also what `stado fleet ...` runs. There is
//! deliberately no logic here: enrollment lived in this binary alone for
//! months, which is how it drifted two minor versions behind the library it
//! shares with `stado` without a single command able to report the gap. A
//! second copy of the parser or the dispatch would be the same mistake with
//! a different shape.

use clap::Parser;
use std::process::ExitCode;
use stado::cli::fleet::{self, FleetCommands};

/// Fleet management for registered Stado hosts.
#[derive(Parser)]
#[command(name = "stado-fleet", version, about)]
struct Cli {
    #[command(subcommand)]
    command: FleetCommands,
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match fleet::run(cli.command).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // A silent failure already printed its own verdict (`doctor`
            // reporting an unhealthy fleet); anything else gets this
            // program's prefix, exactly as before.
            if let Some(message) = error.message.as_deref() {
                eprintln!("stado-fleet: {message}");
            }
            ExitCode::FAILURE
        }
    }
}
