//! Every `stado credentials` verb and its arguments.

use clap::Subcommand;

use crate::cli::secrets::commands::subcommands::{
    CredentialAcquisitionScopeCommands, CredentialBackupCommands, CredentialGrantCommands,
    CredentialItemCommands, CredentialTokenCommands, CredentialVaultCommands,
};

#[derive(Subcommand)]
pub enum SecretsCommands {
    /// Store an item in the selected credential store, reading from STDIN.
    ///
    /// Without --route a write is an owner act: it goes to the owner vault the
    /// store selects. With --route, --consumer, --grant-file and --field it
    /// replaces one field of an existing item under that consumer's own
    /// `rotate:NAME#FIELD` grant, keeping every other field, the kind, the
    /// recipients and the tags; the vault records the consumer as the writer.
    /// A missing grant, an absent or trashed item, or an item a lifecycle or
    /// Weles controls is refused with Skarbiec's answer.
    Put {
        /// Credential item id.
        #[arg(required_unless_present = "role", conflicts_with = "role")]
        name: Option<String>,
        /// Write the one live item carrying the tag `stado:role:<ROLE>`
        /// instead of naming an item, keeping its tags. No item in the role,
        /// or several, is refused. Not with --route: a consumer's rotate grant
        /// names an item, and a scoped consumer cannot look a role up.
        #[arg(long, conflicts_with = "route")]
        role: Option<String>,
        /// Canonical Skarbiec kind. Defaults to the payload's own `kind` when
        /// stdin carries one, else `stado-secret`. An SSH host key stored as a
        /// free-form secret loses the schema's guarantee that both halves are
        /// present, which is how one fleet key ended up shaped unlike its peers.
        #[arg(long = "type", conflicts_with = "route")]
        item_type: Option<String>,
        /// The one field a delegated write replaces. Only with --route.
        #[arg(long, requires = "route")]
        field: Option<String>,
        /// Skarbiec route for a delegated field write. Requires --consumer,
        /// --grant-file and --field; never uses the store administrator.
        #[arg(long, requires_all = ["consumer", "grant_file", "field"])]
        route: Option<String>,
        /// Identity holding the rotate grant for this exact field.
        #[arg(long, requires = "route")]
        consumer: Option<String>,
        /// File holding that consumer's grant.
        #[arg(long, requires = "route")]
        grant_file: Option<String>,
    },
    /// Print one credential item value or one exact string field to stdout,
    /// naming the item by its id or by the role it plays.
    Get {
        /// Credential item id. A value whose stored text is an encrypted
        /// `{"v":…,"c":…}` envelope is refused with the item, the field and
        /// the envelope version, and nothing is printed.
        #[arg(required_unless_present = "role", conflicts_with = "role")]
        name: Option<String>,
        /// Read the one live item carrying the tag `stado:role:<ROLE>` instead
        /// of naming an item. No item in the role, or several, is refused
        /// with the role and the number of items carrying it. With --route the
        /// consumer's grant must be `read:role:<ROLE>#<FIELD>`, and Skarbiec
        /// resolves the role, since a scoped consumer cannot list the vault.
        #[arg(long)]
        role: Option<String>,
        /// Print only this string field. The item id and field remain separate.
        /// A field that is absent or empty is refused.
        #[arg(long)]
        field: Option<String>,
        /// Skarbiec route for a delegated field read. Requires --consumer,
        /// --grant-file and --field; never uses the store administrator.
        #[arg(long, requires_all = ["consumer", "grant_file", "field"])]
        route: Option<String>,
        /// Identity granted access to this exact field.
        #[arg(long, requires = "route")]
        consumer: Option<String>,
        /// File holding that consumer's grant.
        #[arg(long, requires = "route")]
        grant_file: Option<String>,
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
    /// Report whether any key on this machine can still open the vault, and
    /// which key files a restore needs when none can.
    Doctor {
        /// Emit Skarbiec's own JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// The Skarbiec vault: which one this machine resolves to (`show`), which
    /// the fleet holds (`list`), what one holds (`items`), and keeping copies
    /// in step with the owner (`sync`, `retire`).
    ///
    /// Every write and every authoritative read here goes through one file,
    /// and `show` says which; before it the answer lived in a discovery rule
    /// and one environment variable and surfaced only as a refusal.
    Vault {
        #[command(subcommand)]
        command: CredentialVaultCommands,
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
    /// Authenticator seeds in login rows: list whether each row still holds
    /// a seed its account accepts, or enrol one for a row that has none.
    Seed {
        #[command(subcommand)]
        command: CredentialSeedCommands,
    },
    /// A desktop product's Sparkle update key.
    #[command(name = "sparkle-key")]
    SparkleKey {
        #[command(subcommand)]
        command: CredentialSparkleKeyCommands,
    },
    /// A product's code-signing provisioning profiles, stored as fields of
    /// one owner-vault item.
    #[command(name = "signing-profile")]
    SigningProfile {
        #[command(subcommand)]
        command: CredentialSigningProfileCommands,
    },
}

#[derive(Subcommand)]
pub enum CredentialSparkleKeyCommands {
    /// Mint a desktop product's Sparkle update key: an Ed25519 pair stored
    /// under role `<product>-sparkle` (`private_key`, `public_key`), with the
    /// public half written into the app's Info.plist as SUPublicEDKey. A role
    /// that already holds a key is refused without --replace, because copies
    /// installed with the old key accept only updates it signs.
    Mint {
        /// The desktop product, as its release manifest names it (echo-desktop).
        product: String,
        /// The app's Info.plist that ships SUPublicEDKey.
        #[arg(long)]
        info_plist: std::path::PathBuf,
        /// Replace the key the role already holds.
        #[arg(long)]
        replace: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub enum CredentialSigningProfileCommands {
    /// Make or find a product's code-signing provisioning profiles through
    /// the provider's API and store them, base64, as fields of one
    /// owner-vault item.
    Ensure(crate::cli::host::AppleProfileArgs),
}

/// The authenticator seed a login row holds for its account's second factor.
#[derive(Subcommand)]
pub enum CredentialSeedCommands {
    /// Whether login items still hold authenticator seeds their accounts accept.
    List {
        #[arg(long)]
        host: String,
        /// Judge only this login item instead of every login row.
        #[arg(long)]
        login_item: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Enrol an authenticator for one login row and store its seed, so later
    /// sign-ins answer the second factor without a person's phone.
    Enrol {
        #[arg(long)]
        host: String,
        /// The login item whose account should carry an authenticator.
        #[arg(long)]
        login_item: String,
        #[arg(long)]
        json: bool,
    },
}
