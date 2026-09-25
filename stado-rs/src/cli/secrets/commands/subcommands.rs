//! The nested verb groups the top-level surface delegates to.

use clap::Subcommand;

#[derive(Subcommand)]
pub enum CredentialItemCommands {
    /// Store one typed item directly in a host's declared owner vault.
    Put {
        #[arg(long)]
        host: String,
        item: String,
        #[arg(long = "type")]
        item_type: String,
        #[arg(long)]
        json: bool,
    },
    /// Report one host-vault item without revealing its values.
    Show {
        #[arg(long)]
        host: String,
        item: String,
        #[arg(long)]
        field: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Replace one host-vault item's tags, or read them when --tags is omitted.
    Retag {
        #[arg(long)]
        host: String,
        item: String,
        #[arg(long)]
        tags: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Delete one owner-controlled item from a host's vault, for items whose
    /// product or role is retired; Skarbiec keeps the deletion restorable.
    Delete {
        #[arg(long)]
        host: String,
        item: String,
        #[arg(long)]
        json: bool,
    },
    /// Give one host-vault item a new id, keeping its payload, history and
    /// tags; grants naming the old id must be reissued.
    Rename {
        #[arg(long)]
        host: String,
        from: String,
        to: String,
        #[arg(long)]
        json: bool,
    },
    /// Stamp payload fingerprints onto the owner vault's items so the
    /// duplicate report and the duplicate refusal cover every row.
    StampFingerprints {
        #[arg(long)]
        host: String,
        /// Write the fingerprints. Without it, report what the pass would do.
        #[arg(long)]
        apply: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub enum CredentialTokenCommands {
    /// Mint a bounded Skarbiec bearer, or register an existing vault field.
    Mint {
        #[arg(long)]
        host: String,
        consumer: String,
        #[arg(long)]
        capabilities: String,
        #[arg(long)]
        audience: String,
        #[arg(long, default_value_t = 31_536_000)]
        ttl_seconds: u64,
        #[arg(long)]
        replace_capabilities: bool,
        #[arg(long)]
        token_item: Option<String>,
        #[arg(long, requires = "token_item")]
        token_field: Option<String>,
        #[arg(long, conflicts_with = "token_item")]
        raw_token: bool,
        /// With --token-item, the item's own value is written here on the
        /// vault owner, so the consumer's file and the registered bearer agree.
        #[arg(long, conflicts_with = "raw_token")]
        token_file_name: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Deliver an existing bearer between matching declared vault copies, without changing grants.
    Sync {
        consumer: String,
        #[arg(long)]
        from_host: String,
        #[arg(long)]
        host: String,
        #[arg(long)]
        source_token_file: String,
        #[arg(long)]
        token_file: String,
        /// Verify the destination without changing its file.
        #[arg(long)]
        check: bool,
        /// HOST reads FROM_HOST's vault through its own Skarbiec resolver
        /// route; verify the bearer against FROM_HOST's grant, not a local
        /// copy. Refused unless the registry declares that route.
        #[arg(long)]
        shared_vault: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub enum CredentialVaultCommands {
    /// Pull a host's Skarbiec mirror into its declared live vault, or with
    /// --push publish the vault owner's live vault to the mirror.
    Sync {
        #[arg(long)]
        host: String,
        #[arg(long)]
        check: bool,
        /// Publish HOST's live vault to the mirror every copy pulls from.
        /// Run it on the vault owner after a change other copies must see.
        #[arg(long, conflicts_with = "check")]
        push: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub enum CredentialAcquisitionScopeCommands {
    /// Deliver and register an acquisition-scope catalog.
    Sync {
        #[arg(long)]
        host: String,
        source: String,
    },
}

#[derive(Subcommand)]
pub enum CredentialGrantCommands {
    /// Authorize a consumer to read one field of one item.
    #[command(name = "item-read")]
    ItemRead {
        #[arg(long)]
        host: String,
        consumer: String,
        item: String,
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
    /// Revoke a retired consumer whose every capability the stado grant
    /// already holds, leaving one identity on the host's vault.
    #[command(name = "revoke-retired")]
    RevokeRetired {
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
    /// Renew this host's own workload-agent grant, the way the agent does
    /// every ten minutes: at the authoritative vault, with the capabilities
    /// the grant already carries, for thirty days.
    #[command(name = "agent-renew")]
    AgentRenew {
        /// Renew even when more than ten days remain.
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
