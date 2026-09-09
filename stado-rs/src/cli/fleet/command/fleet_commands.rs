//! The `stado fleet` command tree: the published parser surface.

use clap::Subcommand;

use super::{IngressCommands, KeyCommands};

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
    /// Retire a declared fleet. Refused while any target still points at it:
    /// deleting the declaration under a member would leave the document
    /// naming a fleet that does not exist, and `fleet list` refuses exactly
    /// that shape. Reassign the members first.
    Delete {
        /// Declared fleet name.
        name: String,
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
