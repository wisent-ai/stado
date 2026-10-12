//! What `stado service` writes into or about a managed service: a secret
//! into its env file, a file in or out of its home, the runner apphost
//! repair, and the onboarding metadata the registry carries for it.

use clap::Subcommand;

#[derive(Subcommand)]
pub enum ServiceSecretCommands {
    /// Synchronize one Skarbiec field into a service's runtime env file.
    ///
    /// The value is read through the isolated service-verifier grant and carried
    /// in the SSH request body. It is never printed or placed in argv.
    Sync {
        /// Service name, or the host's own name for the unit.
        name: String,
        /// The single registry host to update.
        #[arg(long)]
        host: String,
        /// Skarbiec item containing the secret.
        #[arg(long)]
        item: String,
        /// Exact string field in the Skarbiec item.
        #[arg(long, default_value = "token")]
        field: String,
        /// Environment variable to replace.
        #[arg(long)]
        variable: String,
        /// Runtime env file on the target, absolute or rooted at $HOME.
        #[arg(long)]
        env_file: String,
        /// Restart the service after a successful atomic sync.
        #[arg(long)]
        restart: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub enum ServiceFileCommands {
    /// Synchronize one local file into a managed service's target home.
    ///
    /// The content travels only inside the approved encrypted channel's
    /// request body. It is never printed or placed in an argument vector, and
    /// the destination is replaced atomically with owner-only permissions.
    Sync {
        /// Service whose host-local process uses the file.
        name: String,
        /// The single registry host to update.
        #[arg(long)]
        host: String,
        /// Absolute regular file on this operator host.
        #[arg(long)]
        source_file: String,
        /// File on the target, absolute or rooted at $HOME.
        #[arg(long)]
        target_file: String,
        /// Install mode 0700 instead of 0600.
        #[arg(long)]
        executable: bool,
        #[arg(long)]
        json: bool,
    },

    /// Copy one file OUT of a managed service's target home, byte-exact.
    ///
    /// The opposite direction of `file sync`, and the byte-exact counterpart
    /// of `env show --env-file`. That read sanitizes every value it reports — printable
    /// ASCII, quotes and backslashes replaced, long values clamped — because
    /// its job is to let an operator judge a file without a secret crossing
    /// the channel. The consequence is that it can diagnose a file and can
    /// never reproduce one byte of it, so live operator tooling that exists
    /// only on a host could not be put under version control without copying
    /// it off by hand, outside the approved channel.
    ///
    /// The host hashes the file itself, the bytes travel base64 inside the
    /// same encrypted channel's response, and the digest is recomputed HERE
    /// over the decoded bytes: a payload that lost a chunk decodes into
    /// something shorter and perfectly valid, so only two independently
    /// computed SHA-256s catch it. A mismatch writes nothing and exits
    /// non-zero. `$HOME` confinement and symlink refusal are `env show`'s,
    /// word for word.
    ///
    /// A path that is a directory inside the home answers `directory`, and a
    /// path with no file whose directory exists inside the home answers
    /// `missing`; both carry that directory's entry names (`entries` in
    /// `--json`, one per line on standard error otherwise, a directory's
    /// ending in `/`), so a guessed file name is corrected by the same read.
    /// Names only: no other file is opened and nothing is written.
    Fetch {
        /// Service whose host-local process owns the file.
        name: String,
        /// The single registry host to read.
        #[arg(long)]
        host: String,
        /// File on the target, absolute or rooted at $HOME.
        #[arg(long)]
        source_file: String,
        /// Absolute local path to write the fetched bytes to. Replaced
        /// atomically, owner-only. Omit to report on the file without
        /// keeping a copy.
        #[arg(long)]
        dest_file: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub enum ServiceRunnerRuntimeCommands {
    /// Restore upstream signed macOS GitHub runner apphosts without restarting.
    Repair {
        /// An adopted service launching runsvc.sh or its declared Stado runner launcher.
        name: String,
        #[arg(long)]
        host: String,
        #[arg(long)]
        json: bool,
    },
}

// A clap subcommand enum is constructed once per process from parsed argv, so
// the largest variant costs one stack frame at startup and boxing it would only
// add an allocation and a deref to every match arm.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
pub enum ServiceOnboardingCommands {
    /// Attach central onboarding product metadata to a managed service.
    ///
    /// The metadata becomes part of the canonical Stado registry and is
    /// emitted by `service list --json` for Echo catalog synchronization.
    Set {
        /// Service name, launchd label, or systemd unit.
        name: String,
        /// Registry host that declares the service.
        #[arg(long)]
        host: String,
        #[arg(long)]
        product_id: String,
        #[arg(long)]
        display_name: String,
        #[arg(long)]
        repository: String,
        #[arg(long, value_enum, value_delimiter = ',', num_args = 1..)]
        surfaces: Vec<crate::deploy::service::SurfaceKind>,
        #[arg(long)]
        first_success_fact: String,
        #[arg(long, value_enum, default_value_t = crate::deploy::service::OnboardingKind::Both)]
        onboarding_kind: crate::deploy::service::OnboardingKind,
        #[arg(long, value_enum, default_value_t = crate::deploy::service::OnboardingStatus::Active)]
        status: crate::deploy::service::OnboardingStatus,
        #[arg(long)]
        json: bool,
    },

    /// Registry-managed services carrying Echo onboarding product metadata.
    ///
    /// Emits the versioned JSON envelope accepted by Echo's Stado catalog
    /// synchronization endpoint.
    Catalog,
}
