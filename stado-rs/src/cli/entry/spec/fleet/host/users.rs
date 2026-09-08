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
        #[arg(long, default_value = "gcs", value_parser = ["gcs", "local", "auto"])]
        registry_source: String,
    },
    /// Delete USERNAME from a registry-managed host over SSH.
    Delete {
        username: String,
        /// Registry target name.
        #[arg(long)]
        target: String,
        /// Leave the home directory in place.
        #[arg(long)]
        keep_home: bool,
    },
}
