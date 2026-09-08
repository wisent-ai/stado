//! Which host holds which identity, and the capability handoffs that keep
//! that true.

use clap::Subcommand;

/// Identity bindings: which host holds what, and whether it still does.
#[derive(Subcommand)]
pub(crate) enum IdentityCommands {
    /// Every declared identity binding across the fleet.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Check a binding against the hosts themselves, not the declaration.
    Verify {
        #[arg(long)]
        kind: String,
        #[arg(long)]
        identity: String,
        #[arg(long)]
        json: bool,
    },
    /// Capture on the verified Apple-account holder and store on this worker.
    #[command(hide = true)]
    RelayAppleChallenge {
        #[arg(long)]
        identity: String,
        #[arg(long)]
        authorization_id: String,
        /// Resolve both hosts and their broker/helper without opening a prompt.
        #[arg(long)]
        preflight: bool,
        #[arg(long)]
        json: bool,
    },
    /// Issue Apple login capabilities in the worker's own Weles broker.
    #[command(hide = true)]
    IssueAppleCapabilities {
        #[arg(long)]
        target: String,
        #[arg(long)]
        agent: String,
        #[arg(long)]
        authorization_id: String,
        #[arg(long)]
        ttl_seconds: u64,
        #[arg(long)]
        json: bool,
    },
}
