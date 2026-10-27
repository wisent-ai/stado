//! The `stado placement` command surface: the subcommands clap parses, and the
//! dispatch that hands each one to the operation that answers it.

mod check;
mod evacuate;
mod eviction;
mod moves;
mod role;

pub(crate) use moves::placed_host;

use clap::Subcommand;

use self::eviction::evict;
use self::moves::move_services;
use crate::cli::CmdError;

#[derive(Subcommand)]
pub enum PlacementCommands {
    /// Relocate one complete service group to another registered host, or,
    /// with --authority, move the registry authority role to --to-host.
    Move {
        /// Logical services naming one exact registry placement profile.
        #[arg(required_unless_present = "authority")]
        services: Vec<String>,
        /// Registered destination host.
        #[arg(long)]
        to_host: String,
        /// Move the registry authority role to --to-host, run on that host:
        /// committed through the declared authority while it answers, else
        /// promoted from this host's last-known-good registry and handed to
        /// every other registered host; either way written into the store
        /// this host reads, under the next directory generation.
        #[arg(long, conflicts_with = "services")]
        authority: bool,
        /// Run only the checks the move makes before it fences the source
        /// (units on both hosts, source health and states, destination room,
        /// routing units), against the real hosts, claiming and changing
        /// nothing; with --authority, whether the role would be handed over
        /// or taken over, writing nothing.
        #[arg(long)]
        check: bool,
        /// The vault's owner was rotated to a key --to-host holds: a profile
        /// that carries the fleet vault is moved only with this, which
        /// `stado credentials vault move` passes after it rotated the owner.
        #[arg(long, conflicts_with = "authority")]
        vault_owner_rotated: bool,
        /// Emit the committed transaction report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Empty --from-host onto --to-host: the fleet vault (with
    /// --vault-owner-uid), every placement profile, every role of the host
    /// Stado process, with --authority the registry authority. Every
    /// step is checked first, one refusal refuses the whole evacuation, and
    /// afterwards the registry must place nothing on --from-host.
    Evacuate {
        /// The host everything leaves.
        #[arg(long)]
        from_host: String,
        /// The registered host everything moves to.
        #[arg(long)]
        to_host: String,
        /// Why, recorded with each role's write fence.
        #[arg(long)]
        reason: String,
        /// The vault owner --to-host's keyring holds: the profile carrying
        /// the fleet vault moves through `credentials vault move` with it.
        #[arg(long)]
        vault_owner_uid: Option<String>,
        /// Take the registry authority on --to-host: last, with --from-host's
        /// first role when it holds both, first when it does not answer; run on --to-host.
        #[arg(long)]
        authority: bool,
        /// Stand --to-host by for every profile it does not fully stand by
        /// for, before anything moves; with --check standby's check runs.
        #[arg(long)]
        standby: bool,
        /// Give each fixed route on --from-host the profile `placement profile
        /// draft` prints first, so this run moves it; --check only drafts.
        #[arg(long)]
        profile_fixed_routes: bool,
        /// Pin the schedules pinned to --from-host to --to-host once it is
        /// emptied; a schedule store that does not answer refuses.
        #[arg(long)]
        repin_schedules: bool,
        /// Stop releasing to --from-host each release-control product that
        /// rolls out there once it is emptied (`release policy target remove`).
        #[arg(long)]
        drop_release_targets: bool,
        /// Write each release-controlled install root that would send
        /// --to-host outside its home under `{home}` first (`release policy root repair`).
        #[arg(long)]
        portable_roots: bool,
        /// Run only the checks, claiming, fencing and changing nothing.
        #[arg(long)]
        check: bool,
        #[arg(long)]
        json: bool,
    },
    /// The roles of the host Stado process (the fleet object API): `move`
    /// one to another host as its store and its route.
    #[command(subcommand)]
    Role(role::RoleCommands),
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
        /// Give only the refusals a pass gives before it delivers anything,
        /// and what it would deliver; deliver, stage and write nothing.
        #[arg(long)]
        check: bool,
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
    /// The placement moves the registry holds open: each one's profile,
    /// source and destination host, start time and how long it has been
    /// open. While a move is open every service of its profile is refused to
    /// every consumer, so this names which move holds a refused service.
    Transactions {
        /// Emit the open transactions as JSON.
        #[arg(long)]
        json: bool,
    },
    /// End a placement move the registry still holds open after the process
    /// running it ended without committing or rolling back: undo every step
    /// it may have taken (retire its units on the destination, put back the
    /// destination's state and routes, start the units on the source the
    /// directory still places it on), then release its claim, as the move's
    /// own failure would have. A move still running finds the registry
    /// changed at its commit and rolls back too. Exits non-zero naming each
    /// rollback step that failed.
    Release {
        /// The transaction id `stado placement transactions` lists.
        id: String,
        /// Emit the release report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Every placement change this machine ran, newest first: profile moves,
    /// role moves, authority moves and evacuations, with their receipts or
    /// the refusal that stopped them.
    History {
        #[arg(long)]
        json: bool,
    },
    /// Everything the registry places on HOST: the placement profiles (the
    /// one carrying the fleet vault marked), the roles of the host Stado
    /// process, whether it is the registry authority, the fixed routes, the
    /// profiles it stands by for, release-control products, pinned schedules
    /// and its store's write fence; whether an evacuation has emptied it.
    /// Without HOST, one line per registered host. Changes nothing.
    Holds {
        /// A registered host; every host when omitted.
        host: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

pub async fn dispatch(command: PlacementCommands) -> Result<(), CmdError> {
    match command {
        PlacementCommands::Move {
            services,
            to_host,
            authority,
            check,
            vault_owner_rotated,
            json,
        } => match authority {
            true => crate::cli::resolver::take_authority(&to_host, check, json).await,
            false => {
                move_services(&services, &to_host, check, vault_owner_rotated, json).await
            }
        },
        PlacementCommands::Evacuate {
            from_host,
            to_host,
            reason,
            vault_owner_uid,
            authority,
            standby,
            profile_fixed_routes,
            repin_schedules,
            drop_release_targets,
            portable_roots,
            check,
            json,
        } => {
            let asked = evacuate::Evacuation {
                from: &from_host,
                to: &to_host,
                reason: &reason,
                vault_owner_uid: vault_owner_uid.as_deref(),
                authority,
                check,
                standby,
                profile_fixed: profile_fixed_routes,
                repin_schedules,
                drop_release_targets,
                portable_roots,
            };
            let outcome = evacuate::evacuate(asked, json).await;
            // The run's own report (each step's receipt) is kept beside the
            // moves it made, with its reason; a check changes nothing and is not kept.
            if !check {
                let mut kept = outcome.as_ref().map(serde_json::Value::clone);
                if let Ok(report) = kept.as_mut() {
                    report["reason"] = serde_json::json!(reason);
                    report["state"] = report["status"].clone();
                }
                crate::cli::placement::history::record("evacuate", kept.as_ref().map_err(|error| *error));
            }
            outcome.map(|_| ())
        }
        PlacementCommands::Role(command) => role::run(command).await,
        PlacementCommands::Evict {
            service,
            host,
            json,
        } => evict(&service, &host, json).await,
        PlacementCommands::Standby {
            profile,
            host,
            reason,
            check,
            json,
        } => crate::cli::placement::standby::standby(&profile, &host, &reason, check, json).await,
        PlacementCommands::Profile { command } => {
            crate::cli::placement::profile::dispatch(command).await
        }
        PlacementCommands::Transactions { json } => {
            crate::cli::placement::transactions::list(json).await
        }
        PlacementCommands::Release { id, json } => {
            crate::cli::placement::release::release(&id, json).await
        }
        PlacementCommands::History { json } => crate::cli::placement::history::list(json),
        PlacementCommands::Holds { host: Some(host), json } => evacuate::holds(&host, json).await,
        PlacementCommands::Holds { host: None, json } => evacuate::holds_everywhere(json).await,
    }
}
