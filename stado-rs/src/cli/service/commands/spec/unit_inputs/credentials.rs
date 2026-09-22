//! The credential verbs of `stado service`.

use clap::Subcommand;

/// The credential block of `stado service` verbs. Flattened into
/// [`super::super::ServiceCommands`] directly after
/// [`super::environment::EnvironmentCommands`], so splitting the declaration
/// across files changes neither the accepted command lines nor their order in
/// `--help`.
#[derive(Subcommand)]
pub enum CredentialCommands {
    /// The grants this service's consumers declare, and optionally mint them.
    ///
    /// Bare, it prints what is declared and mints nothing. `--apply` mints
    /// every declared grant through the same path `grant-sync` uses, so a
    /// product's credential need is written where the service is declared
    /// instead of remembered as flags — which is what 26 grants issued from
    /// the shell in one week were the absence of.
    Grants {
        /// Service whose consumers' grants are read.
        name: String,
        /// Only this authorized consumer's grants.
        #[arg(long)]
        consumer: Option<String>,
        /// Authoritative Skarbiec vault on the target, absolute or rooted at $HOME.
        #[arg(long, default_value = "$HOME/.stado/skarbiec.vault.json")]
        vault_file: String,
        /// Lifetime of each minted grant.
        #[arg(long, default_value_t = 2_592_000)]
        ttl_seconds: u64,
        /// Mint the declared grants instead of printing them.
        #[arg(long)]
        apply: bool,
        #[arg(long)]
        json: bool,
    },

    /// Reconcile one Skarbiec consumer grant with an existing owner-only token file.
    ///
    /// The bearer never leaves the managed host: its local Skarbiec reads the
    /// raw file and records only its hash while replacing the declared grant.
    GrantSync {
        /// Service whose host-local deployer uses the grant.
        name: String,
        /// The single registry host to update.
        #[arg(long)]
        host: String,
        /// Exact Skarbiec consumer name.
        #[arg(long)]
        consumer: String,
        /// One complete grant capability; repeat for every capability.
        #[arg(long = "capability", required = true)]
        capabilities: Vec<String>,
        /// Existing raw bearer file on the target, absolute or rooted at $HOME.
        #[arg(long)]
        token_file: String,
        /// Authoritative Skarbiec vault on the target, absolute or rooted at $HOME.
        #[arg(long, default_value = "$HOME/.stado/skarbiec.vault.json")]
        vault_file: String,
        /// Lifetime of the replacement grant.
        #[arg(long, default_value_t = 2_592_000)]
        ttl_seconds: u64,
        /// Grant audience; defaults to the consumer.
        #[arg(long)]
        audience: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// Write one Skarbiec item field into an owner-only raw bearer file.
    ///
    /// `WC_STADO_STORAGE_TOKEN_FILE` has to name a file whose entire content
    /// is the bearer, because `queue/stado_object.rs` resolves a token file
    /// and nothing else. `secret-sync` can put a Skarbiec field into a unit's
    /// env file, and `grant-sync` can reconcile a grant against a token file
    /// that is already on the host, but nothing could create that file. So the
    /// only remaining way to bind a host to the fleet object store was to
    /// hand-copy a secret onto it, which is the one thing the fleet-wide
    /// "everything through Stado" rule exists to prevent. Lacking the file,
    /// charless-mac-mini's queue agent bound its `JobStorage` to a
    /// device-local store instead and published no capacity for seven days
    /// while 74 fleet jobs waited on a host every surface reported as
    /// in-sync -- a fleet claim written to a device store does not fail, it
    /// succeeds where nobody else can see it.
    ///
    /// The value is read through the isolated service-verifier grant and
    /// carried in the SSH request body. It is never printed or placed in argv.
    TokenFileSync {
        /// Service name, or the host's own name for the unit.
        name: String,
        /// The single registry host to update.
        #[arg(long)]
        host: String,
        /// Skarbiec item containing the bearer.
        #[arg(long)]
        item: String,
        /// Exact string field in the Skarbiec item.
        #[arg(long, default_value = "token")]
        field: String,
        /// Destination bearer file on the target, absolute or rooted at $HOME.
        #[arg(long)]
        token_file: String,
        #[arg(long)]
        json: bool,
    },

    /// Verify a managed service's bearer against a read-only loopback endpoint.
    ///
    /// With `--repair`, a failed check atomically synchronizes the secret,
    /// restarts the unit, and checks the endpoint once more.
    AuthCheck {
        /// Service name, or the host's own name for the unit.
        name: String,
        /// The single registry host to check.
        #[arg(long)]
        host: String,
        /// Skarbiec item containing the bearer. Required unless the bearer
        /// is read from the unit's own runtime environment with --variable
        /// and --env-file instead.
        #[arg(long)]
        item: Option<String>,
        /// Host-side Skarbiec consumer used to read --item (defaults to the
        /// host's own selection).
        #[arg(long)]
        consumer: Option<String>,
        /// Token file for --consumer.
        #[arg(long)]
        token_file: Option<String>,
        /// Exact string field in the Skarbiec item.
        #[arg(long, default_value = "token")]
        field: String,
        /// Read-only loopback HTTP endpoint that requires authentication.
        #[arg(long)]
        url: String,
        /// Send an empty JSON POST instead of a GET; useful for auth-first APIs.
        #[arg(long)]
        post_empty_json: bool,
        /// Treat this exact HTTP status as proof that authentication passed.
        #[arg(long)]
        expect_status: Option<u16>,
        /// On failure, synchronize the secret, restart, and check again.
        #[arg(long)]
        repair: bool,
        /// If repair still fails, stop the unmanaged process owning the URL port.
        #[arg(long, requires = "repair")]
        take_over_listener: bool,
        /// Environment variable holding the bearer. With --item omitted this
        /// names the assignment auth-check reads from --env-file; with
        /// --repair it is the assignment synchronized from the item.
        #[arg(long)]
        variable: Option<String>,
        /// Runtime env file holding (or, with --repair, receiving) the
        /// bearer assignment named by --variable.
        #[arg(long)]
        env_file: Option<String>,
        #[arg(long)]
        json: bool,
    },
}
