//! The grant and backup verb groups: who may read what in a host's vault,
//! and how a host's local replica compares with the store it mirrors.

use clap::Subcommand;

#[derive(Subcommand)]
pub enum CredentialGrantCommands {
    /// Add to a consumer's grant the read of one field of the item that plays a role: by the role (`read:role:<role>#<field>`, what `credentials get --role --route` asks for) and by the item playing it now. --token-file is the consumer's bearer file on the host.
    Add {
        #[arg(long)]
        host: String,
        consumer: String,
        #[arg(long)]
        role: String,
        #[arg(long)]
        field: String,
        #[arg(long)]
        token_file: String,
        #[arg(long)]
        json: bool,
    },
    /// Merge retired consumer capabilities into Stado's existing bearer.
    #[command(name = "consolidate")]
    Consolidate {
        #[arg(long)]
        host: String,
        #[arg(long = "from", required = true)]
        sources: Vec<String>,
        #[arg(long)]
        token_file: String,
        #[arg(long)]
        json: bool,
    },
    /// Bind the stado grant back to the bearer file the fleet holds, when
    /// that file no longer opens it: same capabilities, audience and expiry.
    ///
    /// The recovery for every credential read answering "consumer not
    /// authorized to read item field" after the owner's grant record and the
    /// fleet's bearer file diverged. Refused when the file already opens the
    /// grant; verified after.
    #[command(name = "rebind")]
    Rebind {
        /// The host that owns the vault.
        #[arg(long)]
        host: String,
        /// The stado bearer file on that host (absolute path).
        #[arg(long)]
        token_file: String,
        #[arg(long)]
        json: bool,
    },
    /// Revoke a retired consumer whose every capability the stado grant
    /// already holds, leaving one identity on the host's vault. Refused for
    /// `stado` and for a consumer holding anything the stado grant lacks.
    Revoke {
        #[arg(long)]
        host: String,
        consumer: String,
        #[arg(long)]
        json: bool,
    },
    /// Report one consumer's recorded grant.
    Show {
        #[arg(long)]
        host: String,
        consumer: String,
        #[arg(long)]
        token_file: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Renew this host's own workload-agent grant until revoked, the way
    /// the agent's tick does when the vault still records an end for it: at
    /// the authoritative vault, with the capabilities the grant already
    /// carries. The consumer is the one the workload agent's configuration names.
    Renew {
        /// Re-issue even when the grant already lives until revoked.
        #[arg(long)]
        force: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub enum CredentialBackupCommands {
    /// Classify a host's local replica against the store it mirrors.
    Audit {
        #[arg(long)]
        host: String,
        #[arg(
            long = "object",
            value_name = "STADO_URI",
            conflicts_with = "reclaim_twins"
        )]
        objects: Vec<String>,
        #[arg(
            long = "inventory-namespace",
            value_name = "NAMESPACE",
            conflicts_with = "reclaim_twins"
        )]
        inventory_namespaces: Vec<String>,
        #[arg(long = "reclaim-twins")]
        reclaim_twins: bool,
        #[arg(long, requires = "reclaim_twins")]
        apply: bool,
        #[arg(long)]
        json: bool,
    },
}
