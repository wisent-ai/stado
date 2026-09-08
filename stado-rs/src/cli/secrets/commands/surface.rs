//! Every `stado secrets` verb and its arguments.

use clap::Subcommand;

use crate::cli::secrets::commands::subcommands::{
    CredentialAcquisitionScopeCommands, CredentialBackupCommands, CredentialGrantCommands,
    CredentialItemCommands, CredentialTokenCommands, CredentialVaultCommands,
};

#[derive(Subcommand)]
pub enum SecretsCommands {
    /// Store an item in the selected credential store, reading from STDIN.
    Put {
        /// Credential item id.
        name: String,
        /// Canonical Skarbiec kind. Defaults to the payload's own `kind` when
        /// stdin carries one, else `stado-secret`. An SSH host key stored as a
        /// free-form secret loses the schema's guarantee that both halves are
        /// present, which is how one fleet key ended up shaped unlike its peers.
        #[arg(long = "type")]
        item_type: Option<String>,
    },
    /// Print one credential item value or one exact string field to stdout.
    Get {
        /// Credential item id.
        name: String,
        /// Print only this string field. The item id and field remain separate.
        #[arg(long)]
        field: Option<String>,
    },
    /// List metadata for items visible to the credential-store admin.
    Ls {
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Delete one item from the selected credential store.
    Rm {
        /// Credential item id.
        name: String,
    },
    /// Move every credential to a new backend and commit the selector.
    Migrate {
        /// Destination selector. Omit when STADO_CREDENTIALS_STORE changed.
        #[arg(long)]
        to: Option<String>,
    },
    /// Mint one request-only bootstrap token directly into an owner-only file.
    #[command(name = "mint-acquisition-token")]
    MintAcquisitionToken {
        /// Exact consumer identity.
        consumer: String,
        /// Exact existing Skarbiec item id.
        item: String,
        /// Exact string field the consumer may request.
        field: String,
        /// New token file. Refuses to overwrite an existing path.
        output: String,
    },
    /// Report whether any key on this machine can still open the vault, and
    /// which key files a restore needs when none can.
    Doctor {
        /// Emit Skarbiec's own JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Report which vault this machine's credential operations resolve to,
    /// and why.
    ///
    /// Every write and every authoritative read here goes through one file,
    /// and until this command existed nothing said which — the answer lived
    /// in a discovery rule and one environment variable, and it surfaced only
    /// as a refusal from whatever command hit it. On 2026-09-05 that was
    /// `stado repair stado --step release-verifier`, after two vaults on this
    /// machine had been claiming one owner for long enough to close the
    /// fleet's release publication boundary.
    ///
    /// Exits non-zero when nothing resolves, so a script can gate on it.
    Vault {
        /// Host-vault operation. Omit to report this machine's authority.
        #[command(subcommand)]
        command: Option<CredentialVaultCommands>,
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// List nonsecret item metadata from one owner-controlled vault file.
    ///
    /// With `--host` the vault is the one THAT host holds, read through the
    /// registry's own channel with the same read-only `skarbiec list` the
    /// fleet's vault inventory already uses. A remote host's vault is a
    /// separate store from this machine's — its capability routes, its
    /// capability state and its items are all its own — and nothing else in
    /// the product could answer "does that host hold this item" without
    /// copying an encrypted vault around.
    ///
    /// Names, kinds, states and tags only, never a field value.
    #[command(name = "inspect-vault")]
    InspectVault {
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
    /// Recreate Weles internal authorities in the canonical owner vault from
    /// surviving owner credentials.
    #[command(name = "bootstrap-weles")]
    BootstrapWeles {
        /// Emit only the recreated item names as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Merge the retired Weles-dedicated vault into the canonical owner vault.
    ///
    /// Copies only the ids the canonical vault does not already hold, reports
    /// one outcome per item, reads the side vault and never writes to it, and
    /// prints item names, field-level reasons and counts — never a value.
    #[command(name = "adopt-weles-vault")]
    AdoptWelesVault {
        /// Emit the per-item outcomes as JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Inventory credentials recoverable from agent transcripts. Reports names
    /// and counts, never values.
    Harvest {
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
        /// Restore one exact name into the selected store, newest observation first.
        /// The value is never printed.
        #[arg(long)]
        restore: Option<String>,
        /// Also scan payloads that merely quoted a file. Those are source code,
        /// so their names are usually identifiers, not credentials in use.
        #[arg(long)]
        all: bool,
    },
    /// Test unlock phrases found in transcripts against a local or remote
    /// vault, reporting which source name worked. Never prints a phrase.
    TryUnlock {
        /// Registry host holding the protected vault. Omit for the local vault.
        #[arg(long)]
        host: Option<String>,
        /// Test only the macOS Keychain entry, without replaying transcript phrases.
        #[arg(long, requires = "host")]
        keychain_only: bool,
    },
    /// Credential item operations on the host that owns the vault.
    Item {
        #[command(subcommand)]
        command: CredentialItemCommands,
    },
    /// Credential token operations on the host that owns the vault.
    Token {
        #[command(subcommand)]
        command: CredentialTokenCommands,
    },
    /// Which Skarbiec vaults the fleet holds.
    Vaults {
        /// Ask one host instead of the whole registry.
        #[arg(long)]
        host: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Acquisition-scope operations on a host vault.
    #[command(name = "acquisition-scopes")]
    AcquisitionScopes {
        #[command(subcommand)]
        command: CredentialAcquisitionScopeCommands,
    },
    /// Consumer grant operations on a host vault.
    Grant {
        #[command(subcommand)]
        command: CredentialGrantCommands,
    },
    /// Backup operations associated with credential custody.
    Backup {
        #[command(subcommand)]
        command: CredentialBackupCommands,
    },
    /// Whether login items still hold authenticator seeds their accounts accept.
    #[command(name = "seed-freshness")]
    SeedFreshness {
        #[arg(long)]
        host: String,
        /// Judge only this login item instead of every login row.
        #[arg(long)]
        login_item: Option<String>,
        #[arg(long)]
        json: bool,
    },
}
