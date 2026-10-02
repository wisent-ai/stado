//! Local macOS and Linux accounts on registry-managed hosts.

use clap::Subcommand;

#[derive(Subcommand)]
pub(crate) enum HostUserCommands {
    /// Create USERNAME on selected registry-managed hosts over SSH.
    Create {
        username: String,
        /// Registry target name. Repeat to provision several hosts.
        #[arg(long)]
        target: Vec<String>,
        /// Provision every kind=local registry target with SSH configured.
        #[arg(long)]
        all: bool,
        /// Account display name.
        #[arg(long)]
        full_name: Option<String>,
        /// Absolute login shell; host OS default if omitted.
        #[arg(long, default_value = "")]
        shell: String,
        /// Create an administrator account instead of a standard user.
        #[arg(long)]
        admin: bool,
        /// Require the new user to change the initial password on first login.
        #[arg(long)]
        require_password_change: bool,
        /// Validate and list targets without connecting.
        #[arg(long)]
        dry_run: bool,
        /// Where the registry is read from: the canonical store, the bundled
        /// file, or the store with the bundled file behind it.
        #[arg(long, value_enum, default_value = "remote")]
        registry_source: crate::cli::host::RegistrySource,
        /// Emit one JSON document listing every host's outcome.
        #[arg(long)]
        json: bool,
    },
    /// Delete USERNAME from a registry-managed host over SSH. The account,
    /// and its home directory unless --keep-home, cannot be restored, so the
    /// username is repeated with --confirm.
    Delete {
        username: String,
        /// Registry target name.
        #[arg(long)]
        target: String,
        /// Leave the home directory in place.
        #[arg(long)]
        keep_home: bool,
        /// The username again; anything else is refused before the host is contacted.
        #[arg(long)]
        confirm: String,
        /// Emit the outcome as JSON.
        #[arg(long)]
        json: bool,
    },
}
