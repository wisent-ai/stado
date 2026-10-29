//! `stado workload harness status|install KIND --target HOST`: the harness a
//! session kind declares, on one host.

use clap::Subcommand;

use crate::cli::workload::catalog::workload;
use crate::cli::workload::runners::{harness_status, install_harness};
use crate::cli::CmdError;

#[derive(Subcommand)]
pub enum HarnessCommands {
    /// Whether HOST runs the same harness this machine runs: both versions
    /// and the managed path.
    Status {
        kind: String,
        #[arg(long)]
        target: String,
        /// Emit the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Bring the harness version this machine runs to HOST's managed path:
    /// this machine's copy for a host on its platform, the build its maker
    /// published for any other platform (checked against the release's
    /// SHA-256 list). Checked there by its own `--version` before it replaces
    /// the one in place. A host already running the same version is left
    /// alone.
    Install {
        kind: String,
        #[arg(long)]
        target: String,
        /// Emit the report as JSON.
        #[arg(long)]
        json: bool,
    },
}

pub(super) async fn dispatch(command: HarnessCommands) -> Result<(), CmdError> {
    match command {
        HarnessCommands::Status { kind, target, json } => {
            harness_status(workload(&kind)?, &target, json).await
        }
        HarnessCommands::Install { kind, target, json } => {
            install_harness(workload(&kind)?, &target, json).await
        }
    }
}
