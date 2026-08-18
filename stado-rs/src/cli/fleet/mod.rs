//! `stado fleet` — enrollment, fleet membership, SSH-key custody and worker
//! diagnosis for the registered Stado hosts.
//!
//! This is the whole implementation, not a wrapper: the `stado_fleet` binary
//! parses the same [`FleetCommands`] and calls the same [`run`]. Adding a
//! machine used to live only in that separate binary, which meant the one
//! command an operator needs first was invisible to `stado --help`, to the
//! dashboard's operator console (it executes `stado`, and only `stado`), and
//! to anything else built on the main CLI. One implementation behind two
//! entry points is the fix; a second copy of the enrollment logic would
//! reintroduce exactly the drift that made the `stado_fleet` binary two minor
//! versions stale while nobody noticed.
//!
//! The fleet's blind spot before `doctor` existed: a worker could sit in a
//! crash loop with no command able to say why. `doctor` closes that — it
//! verifies the agent credential grant against the configured allowlist,
//! probe-reads every declared secret field without printing values, and
//! reports per-target beacon and capacity presence, all through Stado's own
//! reads.

use clap::Subcommand;

use super::{CmdError, CLICK_ERROR_CODE};

pub mod doctor;
pub mod enroll;
pub mod fleets;
pub mod ingress;
pub mod invite;
pub mod key;
pub mod ops;
#[cfg(test)]
mod tests;

/// Fleet management for registered Stado hosts.
#[derive(Subcommand)]
pub enum FleetCommands {
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
        /// Install the fleet's public key into the machine's
        /// ~/.ssh/authorized_keys before probing it — the `adopt` method. Use
        /// this for a machine that is not in the fleet yet, whenever you can
        /// already open an SSH session to it some other way (a loaded ssh
        /// agent, one of your own keys, or the account password, which OpenSSH
        /// asks for itself). Without it, enroll assumes the fleet's key is
        /// already in authorized_keys there.
        #[arg(long)]
        install_key: bool,
        /// Install the agent on the machine after registering it.
        #[arg(long)]
        bootstrap: bool,
    },
    /// Mint an invite: something the machine's owner runs, no access needed.
    Invite {
        /// Registry target name to reserve; derived from the invite id when
        /// omitted.
        #[arg(long)]
        name: Option<String>,
        /// How long the invite stays usable: a number plus s, m, h or d.
        #[arg(long, default_value = "24h")]
        expires: String,
        /// How many machines may redeem the invite.
        #[arg(long, default_value_t = 1)]
        uses: u64,
        /// Skip the control-point probe and issue the pasteable offline
        /// fragment, which needs no HTTP route at all. Without the flag an
        /// unreachable control point selects this mode anyway, and says why.
        #[arg(long)]
        offline: bool,
        /// Emit the machine-readable document instead of the report.
        #[arg(long)]
        json: bool,
    },
    /// List minted invites with the state each is actually in.
    Invites {
        /// Emit the machine-readable document instead of the table.
        #[arg(long)]
        json: bool,
    },
    /// Retire an invite so it can no longer be redeemed.
    RevokeInvite {
        /// Invite id as printed by `invite` and `invites`.
        id: String,
    },
    /// Stand up, inspect or tear down the public entrance the one-line invite
    /// mode needs — a narrow enrollment listener behind a Cloudflare quick
    /// tunnel, with no Cloudflare account, token or DNS record involved.
    #[command(subcommand)]
    Ingress(IngressCommands),
    /// Every way a machine can be added, and whether this fleet allows it.
    Methods {
        /// Emit the machine-readable document instead of the table.
        #[arg(long)]
        json: bool,
    },
    /// Announce this machine to the fleet (run on the machine being added).
    Join,
    /// List unanswered join requests.
    Pending {
        /// Emit the machine-readable document instead of the table.
        #[arg(long)]
        json: bool,
    },
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
    /// SSH host keys in the globally selected credential store.
    #[command(subcommand)]
    Key(KeyCommands),
}

/// The three things an operator does to the fleet's public entrance. There is
/// no `restart`: a quick tunnel comes back under a different address, so the
/// operation that reads like "the same entrance again" is exactly the one that
/// silently invalidates every invitation already handed out. `down` then `up`
/// says what happened.
#[derive(Subcommand)]
pub enum IngressCommands {
    /// Start the narrow enrollment listener and a tunnel in front of it,
    /// verify the public address from the internet, then publish it.
    Up {
        /// Loopback port for the listener. Chosen automatically when omitted;
        /// a port already in use is refused, never adopted.
        #[arg(long)]
        port: Option<u16>,
        /// Use a named tunnel on the fleet's own domain instead of a quick
        /// one. Refused today: the Cloudflare API token it needs does not
        /// exist in the vault.
        #[arg(long)]
        named: bool,
    },
    /// What is published, whether it still answers, and how old it is.
    Status {
        /// Emit the machine-readable document instead of the report.
        #[arg(long)]
        json: bool,
    },
    /// Close the tunnel, stop the listener, and unpublish the address.
    Down,
}

#[derive(Subcommand)]
pub enum KeyCommands {
    /// Move an existing private key into the credential store (never printed).
    Add {
        /// Registry target the key belongs to.
        target: String,
        /// Private key file removed after verified storage.
        #[arg(long)]
        from: String,
    },
    /// List stored SSH host keys (metadata only).
    Ls,
    /// Remove a target's SSH key from the credential store.
    Rm {
        /// Registry target.
        target: String,
    },
    /// Install the stored public key into the target's authorized_keys.
    Install {
        /// Registry target.
        target: String,
    },
    /// Verify the stored key opens the channel to the target.
    Check {
        /// Registry target.
        target: String,
    },
    /// Generate a fresh ed25519 pair for the target into the credential store.
    Generate {
        /// Registry target.
        target: String,
    },
    /// Rotate the target's key end to end, with rollback on failure.
    Rotate {
        /// Registry target.
        target: String,
    },
}

/// Run one fleet command.
///
/// The commands report in the fleet's own vocabulary — `Ok(true)` is "done",
/// `Ok(false)` is "ran, and the fleet is not healthy" (only `doctor` says
/// that), `Err` is a failure with a sentence for the operator. The exit
/// contract is the CLI's: a verdict of `false` exits non-zero in silence,
/// because `doctor` already printed the failing rows and a second, classified
/// diagnosis line would contradict a command that deliberately said its own
/// last word.
pub async fn run(command: FleetCommands) -> Result<(), CmdError> {
    let outcome = execute(command).await;
    match outcome {
        Ok(true) => Ok(()),
        Ok(false) => Err(CmdError::silent(CLICK_ERROR_CODE)),
        Err(message) => Err(CmdError::click(message)),
    }
}

/// Dispatch to the implementation of one command, keeping the fleet's
/// `Result<bool, String>` verdict intact for [`run`] to translate.
async fn execute(command: FleetCommands) -> Result<bool, String> {
    match command {
        FleetCommands::Doctor { json, fleet } => doctor::run(json, fleet.as_deref()).await,
        FleetCommands::List { json } => fleets::list(json).await,
        FleetCommands::Status { name } => fleets::status(&name).await,
        FleetCommands::Create { name, notes } => ops::create(&name, &notes).await,
        FleetCommands::Assign { target, fleet } => ops::assign(&target, &fleet).await,
        FleetCommands::Enroll {
            name,
            ssh,
            kind,
            fleet,
            bootstrap,
            install_key,
        } => {
            ops::enroll(
                &name,
                Some(&ssh),
                &kind,
                fleet.as_deref(),
                bootstrap,
                install_key,
            )
            .await
        }
        FleetCommands::Invite {
            name,
            expires,
            uses,
            offline,
            json,
        } => invite::invite(name.as_deref(), &expires, uses, offline, json).await,
        FleetCommands::Invites { json } => invite::invites(json).await,
        FleetCommands::RevokeInvite { id } => invite::revoke_invite(&id).await,
        FleetCommands::Ingress(sub) => match sub {
            IngressCommands::Up { port, named } => ingress::up(port, named).await,
            IngressCommands::Status { json } => ingress::status(json).await,
            IngressCommands::Down => ingress::down().await,
        },
        FleetCommands::Methods { json } => enroll::catalog::methods(json).await,
        FleetCommands::Join => enroll::join().await,
        FleetCommands::Pending { json } => enroll::pending(json).await,
        FleetCommands::Approve { hostname, fleet } => {
            enroll::approve(&hostname, fleet.as_deref()).await
        }
        FleetCommands::Reject { hostname } => enroll::reject(&hostname).await,
        FleetCommands::Catalog { json } => enroll::catalog::catalog(json).await,
        FleetCommands::Key(sub) => {
            let runner = crate::deploy::production_runner();
            match sub {
                KeyCommands::Add { target, from } => key::add(&runner, &target, &from).await,
                KeyCommands::Ls => key::ls().await,
                KeyCommands::Rm { target } => key::rm(&target).await,
                KeyCommands::Install { target } => key::install(&runner, &target).await,
                KeyCommands::Check { target } => key::check(&runner, &target).await,
                KeyCommands::Generate { target } => key::rotate::generate(&runner, &target).await,
                KeyCommands::Rotate { target } => key::rotate::rotate(&runner, &target).await,
            }
        }
    }
}
