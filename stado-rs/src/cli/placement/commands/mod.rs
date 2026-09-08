//! The `stado placement` command surface: the subcommands clap parses, and the
//! dispatch that hands each one to the operation that answers it.

mod eviction;
mod moves;

use clap::Subcommand;

use self::eviction::evict;
use self::moves::move_services;
use crate::cli::CmdError;

#[derive(Subcommand)]
pub enum PlacementCommands {
    /// Relocate one complete service group to another registered host.
    Move {
        /// Logical services naming one exact registry placement profile.
        #[arg(required = true, num_args = 1..)]
        services: Vec<String>,
        /// Registered destination host.
        #[arg(long)]
        to_host: String,
        /// Emit the committed transaction report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Stop an instance running where the directory places nothing.
    ///
    /// `doctor`'s placement row tells the operator to end exactly this, and
    /// until now no command could: `service stop` refuses a host that
    /// declares no such unit, which is the definition of the squatter it is
    /// asked to remove. The port comes from the directory, so an instance can
    /// only be evicted from a host the directory does NOT place it on.
    Evict {
        /// Logical service the directory declares.
        service: String,
        /// Registered host holding the port it should not hold.
        #[arg(long)]
        host: String,
        /// Emit the eviction report as JSON.
        #[arg(long)]
        json: bool,
    },
}

pub async fn dispatch(command: PlacementCommands) -> Result<(), CmdError> {
    match command {
        PlacementCommands::Move {
            services,
            to_host,
            json,
        } => move_services(&services, &to_host, json).await,
        PlacementCommands::Evict {
            service,
            host,
            json,
        } => evict(&service, &host, json).await,
    }
}
