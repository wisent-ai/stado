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
    /// Make or find a product's Apple provisioning profiles through the App
    /// Store Connect API and store them, base64, as fields of one owner-vault item.
    AppleProfile(crate::cli::host::AppleProfileArgs),
    /// Host primitive used by `show`: read a `skarbiec get --json` document
    /// on stdin and print each field's length and SHA-256, never a value.
    #[command(name = "summarize-local", hide = true)]
    SummarizeLocal,
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
        /// Store a fresh bearer as the `token` field of this owner-vault item
        /// (kind `token`) when the item is absent, then register the item's
        /// value; a repeated run registers the same stored bearer.
        #[arg(long, conflicts_with_all = ["token_item", "raw_token", "token_file_name"])]
        store_item: Option<String>,
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
    /// Vault-owner primitive used by `mint --token-item`: register ITEM#FIELD
    /// with `skarbiec grant issue` and keep it at STADO_TOKEN_DESTINATION.
    #[command(name = "register-item-local", hide = true)]
    RegisterItemLocal {
        skarbiec: String,
        item: String,
        field: String,
        /// The `grant issue …` arguments Skarbiec receives.
        #[arg(last = true)]
        arguments: Vec<String>,
    },
    /// Host primitive used by `sync`: export, install or check one bearer
    /// against the declared consumer grant, reading an export on stdin.
    #[command(name = "custody-local", hide = true)]
    CustodyLocal {
        operation: String,
        vault: String,
        consumer: String,
        file: String,
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
