//! The `stado placement` command surface: the subcommands clap parses, and the
//! dispatch that hands each one to the operation that answers it.

mod delegate;
mod eviction;
mod moves;

pub(crate) use moves::placed_host;

use clap::Subcommand;

use self::eviction::evict;
use self::moves::move_services;
use crate::cli::CmdError;

#[derive(Subcommand)]
pub enum PlacementCommands {
    /// Relocate one complete service group to another registered host, or,
    /// with --authority, take the registry authority role on --to-host while
    /// the declared authority does not answer.
    Move {
        /// Logical services naming one exact registry placement profile.
        #[arg(required_unless_present = "authority")]
        services: Vec<String>,
        /// Registered destination host.
        #[arg(long)]
        to_host: String,
        /// Take the registry authority role on --to-host, run on that host:
        /// promote its last-known-good registry under the next directory
        /// generation, write it into the store it reads, and hand it to every
        /// other registered host. Refused while the authority answers.
        #[arg(long, conflicts_with = "services")]
        authority: bool,
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
    /// Make a registered host a place this profile may move to: deliver each
    /// service's program (a managed program now; a release-controlled tree
    /// through the host's own agent), stage each unit rendered from the
    /// service catalog where the move starts it (installed, not loaded, not
    /// declared), and write the host into the profile and the directory once
    /// every unit is staged. Idempotent; a unit already running on the host is
    /// refused as a second active copy, and a pass that stops at a pending
    /// rollout says so and the next pass continues.
    Standby {
        /// The placement profile to prepare the host for.
        profile: String,
        /// Registered host that must stand by for it.
        #[arg(long)]
        host: String,
        /// Why this host must stand by for the profile.
        #[arg(long)]
        reason: String,
        /// Emit the receipt as JSON.
        #[arg(long)]
        json: bool,
    },
    /// List, show, declare and remove the placement profiles the registry
    /// holds: the service groups `standby` prepares a host for and `move`
    /// relocates.
    Profile {
        #[command(subcommand)]
        command: crate::cli::placement::profile::ProfileCommands,
    },
}

pub async fn dispatch(command: PlacementCommands) -> Result<(), CmdError> {
    match command {
        PlacementCommands::Move {
            services,
            to_host,
            authority,
            json,
        } => match authority {
            true => crate::cli::resolver::take_authority(&to_host, json).await,
            false => move_services(&services, &to_host, json).await,
        },
        PlacementCommands::Evict {
            service,
            host,
            json,
        } => evict(&service, &host, json).await,
        PlacementCommands::Standby {
            profile,
            host,
            reason,
            json,
        } => crate::cli::placement::standby::standby(&profile, &host, &reason, json).await,
        PlacementCommands::Profile { command } => {
            crate::cli::placement::profile::dispatch(command).await
        }
    }
}
