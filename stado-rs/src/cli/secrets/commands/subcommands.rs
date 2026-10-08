//! The nested verb groups the top-level surface delegates to.

use clap::Subcommand;

#[derive(Subcommand)]
pub enum CredentialItemCommands {
    /// Store the secret that plays one role in a host's declared owner vault:
    /// the item carrying stado:role:<ROLE> is rotated, or created under a
    /// random id with that tag.
    Put {
        #[arg(long)]
        host: String,
        #[arg(long)]
        role: String,
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
    /// Return one deleted item from a host vault's trash, undoing delete.
    Restore {
        #[arg(long)]
        host: String,
        item: String,
        #[arg(long)]
        json: bool,
    },
    /// Return one host-vault item from the consumer that wrote it to the
    /// vault owner's control, so retag, rename and delete may change it. Only
    /// control moves; Skarbiec refuses lifecycle- and Weles-managed items.
    Reclaim {
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
    /// Bring the owner vault to Skarbiec's current schema: the v2 envelope,
    /// an item_uid on every item and a payload fingerprint on every active
    /// item, so the duplicate report and the duplicate refusal cover every row.
    Upgrade {
        #[arg(long)]
        host: String,
        /// Write the changes. Without it, report what the pass would do.
        #[arg(long)]
        apply: bool,
        #[arg(long)]
        json: bool,
    },
    /// Make or find a product's code-signing provisioning profiles through
    /// the provider's API and store them, base64, as fields of one
    /// owner-vault item.
    #[command(name = "signing-profile")]
    SigningProfile(crate::cli::host::AppleProfileArgs),
    /// Host primitive used by `show`: read a `skarbiec get --json` document
    /// on stdin and print each field's length and SHA-256, never a value.
    #[command(name = "summarize-local", hide = true)]
    SummarizeLocal,
}

#[derive(Subcommand)]
pub enum CredentialTokenCommands {
    /// Mint a Skarbiec bearer, or register an existing vault field.
    Mint {
        #[arg(long)]
        host: String,
        consumer: String,
        #[arg(long)]
        capabilities: String,
        #[arg(long)]
        audience: String,
        /// Lifetime of the grant in seconds; without it the grant lives until
        /// `skarbiec grant revoke` withdraws it.
        #[arg(long)]
        ttl_seconds: Option<u64>,
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
    /// Report which vault this machine's credential operations resolve to,
    /// and why. Exits non-zero when nothing resolves, so a script can gate
    /// on it.
    Show {
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Which Skarbiec vaults the fleet holds: every registry host, or one.
    List {
        /// Ask one host instead of the whole registry.
        #[arg(long)]
        host: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Nonsecret item metadata (names, kinds, states, tags; never a field
    /// value) from one vault: a local VAULT file, or with `--host` the vault
    /// that host holds, read with the read-only `skarbiec list`.
    Items {
        /// Encrypted Skarbiec vault file. Omit with `--host`.
        vault: Option<String>,
        /// Registry host whose own vault to read instead of a local file.
        #[arg(long)]
        host: Option<String>,
        /// Only report items whose name contains this text.
        #[arg(long = "match")]
        matching: Option<String>,
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
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
    /// Retire one vault file on a machine that reads the vault owner and
    /// holds none: compare it with the owner item by item and report what the
    /// owner lacks; with --apply move those items onto the owner, prove the
    /// owner holds every live item of the copy, and only then remove the file.
    Retire {
        /// The vault file on this machine to retire.
        path: String,
        /// The registry host that holds the fleet vault.
        #[arg(long)]
        owner: String,
        /// Move the missing items and remove the file; without it nothing changes.
        #[arg(long)]
        apply: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub enum CredentialAcquisitionScopeCommands {
    /// Deliver and register an acquisition-scope catalog. Its grants keep
    /// the lifetime their current registration has left, so re-registering
    /// never extends them; a catalog with no current registration needs
    /// --ttl-seconds.
    Sync {
        #[arg(long)]
        host: String,
        source: String,
        /// The grants' lifetime in whole seconds, stated by the operator.
        #[arg(long = "ttl-seconds")]
        ttl_seconds: Option<u64>,
    },
}

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
