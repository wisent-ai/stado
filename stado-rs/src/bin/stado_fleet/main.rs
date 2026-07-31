//! `stado_fleet` — automated fleet management for the registered Stado hosts.
//!
//! The fleet's blind spot today: a worker can sit in a crash loop with no
//! command able to say why. `stado_fleet doctor` closes that — it verifies
//! the agent credential grant against the configured allowlist, probe-reads
//! every declared secret field without printing values, and reports
//! per-target beacon and capacity presence, all through Stado's own reads.

mod doctor;
mod enroll;
mod fleet;
mod key;
mod ops;
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
        /// Scope the fleet section to one named fleet.
        #[arg(long)]
        fleet: Option<String>,
    },
    /// List the fleets declared in the registry with their members.
    List {
        /// Emit the machine-readable document instead of the table.
        #[arg(long)]
        json: bool,
    },
    /// Show live state for the members of one named fleet.
    Status {
        /// Fleet name as declared in the registry `fleets` section.
        name: String,
    },
    /// Declare a new fleet in the canonical registry.
    Create {
        /// Fleet name: a lowercase identifier.
        name: String,
        /// Free-form description of what this fleet is for.
        #[arg(long, default_value = "")]
        notes: String,
    },
    /// Add a registered machine to a declared fleet.
    Assign {
        /// Registry target name (the machine).
        target: String,
        /// Declared fleet name.
        fleet: String,
    },
    /// One-command onboarding: register a machine, optionally fleet it,
    /// optionally install the agent.
    Enroll {
        /// Machine name (a lowercase target identifier).
        name: String,
        /// SSH destination of the machine (user@host) — the verification
        /// channel; the machine is probed before anything is written.
        #[arg(long)]
        ssh: String,
        /// Target kind.
        #[arg(long, default_value = "local")]
        kind: String,
        /// Fleet to place the machine in right away.
        #[arg(long)]
        fleet: Option<String>,
        /// Install the agent on the machine after registering it.
        #[arg(long)]
        bootstrap: bool,
    },
    /// Announce this machine to the fleet (run on the machine being added).
    Join,
    /// List unanswered join requests.
    Pending,
    /// Turn a pending join request into a registered target.
    Approve {
        /// Hostname from the join request.
        hostname: String,
        /// Fleet to place the machine in right away.
        #[arg(long)]
        fleet: Option<String>,
    },
    /// Drop a pending join request.
    Reject {
        /// Hostname from the join request.
        hostname: String,
    },
    /// Print the central enrollment and communication catalog.
    Catalog {
        /// Emit the machine-readable document instead of the table.
        #[arg(long)]
        json: bool,
    },
    /// Host keys in the Skarbiec vault.
    #[command(subcommand)]
    Key(KeyCommands),
}

#[derive(Subcommand)]
enum KeyCommands {
    /// Import an existing private key into the vault (never printed).
    Add {
        /// Registry target the key belongs to.
        target: String,
        /// Path of the private key file to import.
        #[arg(long)]
        from: String,
    },
    /// List vault host keys (metadata only).
    Ls,
    /// Remove a target's vault host key.
    Rm {
        /// Registry target.
        target: String,
    },
    /// Install the vault public key into the target's authorized_keys.
    Install {
        /// Registry target.
        target: String,
    },
    /// Verify the vault key opens the channel to the target.
    Check {
        /// Registry target.
        target: String,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Commands::Doctor { json, fleet } => doctor::run(json, fleet.as_deref()).await,
        Commands::List { json } => fleet::list(json).await,
        Commands::Status { name } => fleet::status(&name).await,
        Commands::Create { name, notes } => ops::create(&name, &notes).await,
        Commands::Assign { target, fleet } => ops::assign(&target, &fleet).await,
        Commands::Enroll {
            name,
            ssh,
            kind,
            fleet,
            bootstrap,
        } => ops::enroll(&name, Some(&ssh), &kind, fleet.as_deref(), bootstrap).await,
        Commands::Join => enroll::join().await,
        Commands::Pending => enroll::pending().await,
        Commands::Approve { hostname, fleet } => enroll::approve(&hostname, fleet.as_deref()).await,
        Commands::Reject { hostname } => enroll::reject(&hostname).await,
        Commands::Catalog { json } => enroll::catalog::catalog(json).await,
        Commands::Key(sub) => {
            let runner = stado::deploy::production_runner();
            match sub {
                KeyCommands::Add { target, from } => key::add(&runner, &target, &from).await,
                KeyCommands::Ls => key::ls().await,
                KeyCommands::Rm { target } => key::rm(&target).await,
                KeyCommands::Install { target } => key::install(&runner, &target).await,
                KeyCommands::Check { target } => key::check(&runner, &target).await,
            }
        }
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
