//! `stado placement role move`: the roles of the host Stado process, which
//! move as their store and their route rather than as units.

use clap::Subcommand;

use crate::cli::CmdError;

#[derive(Subcommand)]
pub enum RoleCommands {
    /// Move a directory service the host Stado process serves (the fleet
    /// object API) to --to-host: fence its store on the active host, copy
    /// the store to --to-host, and name --to-host as the route's active host
    /// in one registry commit; a failed step puts both hosts back.
    Move {
        /// The directory service, e.g. stado-object-api.
        service: String,
        /// The registered host the service moves to.
        #[arg(long)]
        to_host: String,
        /// Why the role moves, recorded with the source's write fence.
        #[arg(long)]
        reason: String,
        /// Carry the registry authority in the same registry: required when
        /// the active host holds it; run on --to-host.
        #[arg(long)]
        with_authority: bool,
        /// Check the route and the store's room, fencing and changing nothing.
        #[arg(long)]
        check: bool,
        #[arg(long)]
        json: bool,
    },
}

pub(super) async fn run(command: RoleCommands) -> Result<(), CmdError> {
    match command {
        RoleCommands::Move { service, to_host, reason, with_authority, check, json } => {
            crate::cli::placement::transfer::move_role(&service, &to_host, &reason, with_authority, check, json).await
        }
    }
}
