//! `stado_fleet` — automated fleet management for the registered Stado hosts.
//!
//! The fleet's blind spot today: a worker can sit in a crash loop with no
//! command able to say why. `stado_fleet doctor` closes that — it verifies
//! the agent credential grant against the configured allowlist, probe-reads
//! every declared secret field without printing values, and reports
//! per-target beacon and capacity presence, all through Stado's own reads.

mod doctor;
#[cfg(test)]
mod tests;

use clap::{Parser, Subcommand};
use std::process::ExitCode;

/// Fleet management for registered Stado hosts.
#[derive(Parser)]
#[command(name = "stado-fleet", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Diagnose worker health: agent grant, secret probes, beacons, capacity.
    Doctor {
        /// Emit the machine-readable report instead of the table.
        #[arg(long)]
        json: bool,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Commands::Doctor { json } => doctor::run(json).await,
    };
    match result {
        Ok(clean) => {
            if clean {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(message) => {
            eprintln!("stado-fleet: {message}");
            ExitCode::FAILURE
        }
    }
}
