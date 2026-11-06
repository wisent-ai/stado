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
    /// Take a directory service the host Stado process serves (the fleet
    /// object API) on this host from an active host that does not answer:
    /// this host's own object API serves it and the registry from this
    /// host's copy is written there. This host must hold the registry
    /// authority (stado placement move --authority). Refused while the active
    /// host answers: then it is moved with its store (role move).
    Take {
        /// The directory service, e.g. stado-object-api.
        service: String,
        /// Why the role is taken, recorded in the placement history.
        #[arg(long)]
        reason: String,
        /// Check the route and this host's object API, writing nothing.
        #[arg(long)]
        check: bool,
        #[arg(long)]
        json: bool,
    },
}

pub(super) async fn run(command: RoleCommands) -> Result<(), CmdError> {
    match command {
        RoleCommands::Move {
            service,
            to_host,
            reason,
            with_authority,
            check,
            json,
        } => {
            crate::cli::placement::transfer::move_role(
                &service,
                &to_host,
                &reason,
                with_authority,
                check,
                json,
            )
            .await
        }
        RoleCommands::Take {
            service,
            reason,
            check,
            json,
        } => crate::cli::placement::transfer::take_role(&service, &reason, check, json).await,
    }
}
