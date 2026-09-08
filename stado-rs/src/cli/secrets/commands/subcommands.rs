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
        #[arg(long, conflicts_with_all = ["raw_token", "token_item"])]
        token_file_name: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub enum CredentialVaultCommands {
    /// Pull a host's Skarbiec mirror into its declared live vault.
    Sync {
        #[arg(long)]
        host: String,
        #[arg(long)]
        check: bool,
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
