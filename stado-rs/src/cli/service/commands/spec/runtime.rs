//! The runtime verbs of `stado service`: image refresh, restart, update,
//! release, and the file and secret deliveries, whose verb groups are
//! declared in `deliveries`.

use clap::{Args, Subcommand};

use super::deliveries::{ServiceFileCommands, ServiceRunnerRuntimeCommands, ServiceSecretCommands};

/// The second block of `stado service` verbs. Flattened into
/// [`super::super::ServiceCommands`], so splitting the declaration across
/// files changes no command line.
#[derive(Subcommand)]
pub enum RuntimeCommands {
    /// The executable image a managed unit's process runs.
    Image {
        #[command(subcommand)]
        command: ServiceImageCommands,
    },

    /// `service image refresh` under its old name, kept only because the
    /// update script `service update` runs on a host calls the host's
    /// installed Stado, which may predate the group.
    #[command(name = "refresh-image", hide = true)]
    RefreshImage(ServiceImageRefreshArgs),

    /// The macOS GitHub runner runtime an adopted service launches.
    RunnerRuntime {
        #[command(subcommand)]
        command: ServiceRunnerRuntimeCommands,
    },

    /// Restart one managed unit, without a full host-recovery pass.
    ///
    /// The command answers when the restarted unit serves the port the
    /// service directory declares for it (SERVING `serving`), or with the
    /// unit's own reason it cannot: its port is held by another job, launchd
    /// holds no process for it, or it died and launchd started another one
    /// (`not_serving`, exit non-zero). A unit with no declared port answers
    /// `no_declared_port` at the kickstart. Elapsed time alone never ends it.
    Restart {
        /// Service name, or the host's own name for the unit.
        name: String,
        /// Restrict to one registry host; omit to restart it everywhere it
        /// is managed. Naming the machine that runs the command finds the
        /// unit in its last-known-good registry copy, not through the object API.
        #[arg(long)]
        host: Option<String>,
        /// Optional loopback URL whose stale listener is stopped before restart.
        #[arg(long)]
        take_over_listener: Option<String>,
        /// Exact per-login recovery label to stop before listener takeover.
        #[arg(long, requires = "take_over_listener")]
        recovery_unit: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// Move an already-managed service onto a new artifact version.
    ///
    /// `deploy` installs a unit that is not yet managed and refuses to touch one
    /// that is. This is the other half: the unit is left exactly as it is, the
    /// new version is placed beside the running one and `current` is relinked,
    /// so the change takes effect on the next restart and a rollback is a
    /// relink rather than a redeploy.
    Update {
        /// Service name as the registry manages it.
        name: String,
        #[arg(long)]
        host: String,
        /// Published artifact to install.
        #[arg(long, conflicts_with = "from_archive")]
        from_artifact: Option<String>,
        /// Local release archive to install, for a bundle that no object store
        /// the fleet shares is carrying yet.
        #[arg(long, conflicts_with = "from_artifact")]
        from_archive: Option<String>,
        /// Point `current` back at a version directory already on the host.
        #[arg(long, conflicts_with_all = ["from_artifact", "from_archive"])]
        rollback_to: Option<String>,
        /// Reconcile and verify the live kernel image after installation.
        #[arg(long)]
        refresh_image: bool,
        #[arg(long)]
        json: bool,
    },

    /// Install and activate one service release, rolling back on failed readiness.
    Release {
        /// Service name as the registry manages it.
        name: String,
        #[arg(long)]
        host: String,
        /// Product in registry.release_control.
        #[arg(long)]
        product: String,
        /// Exact desired semantic version to activate.
        #[arg(long)]
        version: String,
        /// Optional loopback HTTP endpoint that must answer after restart.
        #[arg(long, requires = "readiness_timeout_seconds")]
        readiness_url: Option<String>,
        /// Seconds the readiness URL has to answer. Required with
        /// `--readiness-url`; no window is assumed.
        #[arg(long, requires = "readiness_url")]
        readiness_timeout_seconds: Option<u64>,
        /// Reload a system LaunchDaemon's unit definition before readiness.
        ///
        /// `kickstart` reuses launchd's cached ProgramArguments. Use this when
        /// the plist was repointed from a legacy path to managed `current`.
        #[arg(long)]
        reload_unit: bool,
        /// Require readiness JSON field `releaseVersion` or `build.version` to equal `--version`.
        #[arg(long)]
        require_release_version: bool,
        /// Replace one legacy user LaunchAgent atomically with this release.
        ///
        /// The legacy unit is restored when activation fails. Its plist is
        /// deleted only after exact readiness passes.
        #[arg(long)]
        supersede_unit: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// What a managed unit actually runs: its program, arguments and unit file.
    ///
    /// `env` answers what the unit runs *with*; nothing answered what it runs.
    /// That gap is why a restart that dropped every argument after the program
    /// path looked like a broken service rather than a broken restart.
    Show {
        /// Service name, or the host's own name for the unit.
        name: String,
        /// Restrict to one registry host.
        #[arg(long)]
        host: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// Stop one managed unit, including a process the unit no longer owns.
    ///
    /// `retire` removes a service from management; this only stops it. The
    /// difference matters when a restart has previously spawned the program
    /// outside its own label: launchctl then disowns it, the stale process
    /// keeps the port, and every later restart dies on "address already in
    /// use" while the broken instance serves on.
    ///
    /// On Linux a refused systemctl stop names the unit, scope, exit status
    /// and host error. The command then reads the unit's actual state; an
    /// active, transitioning or unreadable answer is not a completed stop.
    Stop {
        /// Service name, or the host's own name for the unit.
        name: String,
        /// Restrict to one registry host; omit to stop it everywhere it is managed.
        #[arg(long)]
        host: Option<String>,
        /// Optional loopback URL whose disowned listener must also be gone.
        #[arg(long)]
        listener_url: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// One Skarbiec field in a service's runtime env file.
    Secret {
        #[command(subcommand)]
        command: ServiceSecretCommands,
    },

    /// One file in a managed service's target home, copied in or out
    /// byte-exact.
    File {
        #[command(subcommand)]
        command: ServiceFileCommands,
    },
}

/// The executable image a managed unit's process runs.
#[derive(Subcommand)]
pub enum ServiceImageCommands {
    /// Put one unit back on the file its `ProgramArguments` name, and prove
    /// it landed.
    ///
    /// The verb behind `registry doctor`'s `stale-unit-image` row. It refuses
    /// a unit that is not stale, naming the identity it found, because a
    /// command that restarts whatever it is pointed at is a restart button.
    /// It re-reads the image afterwards and exits non-zero if the restart did
    /// not change it: launchd re-execs the declared path, and a process can
    /// respawn under `KeepAlive` straight back onto the same unlinked inode
    /// it had just left.
    ///
    /// One unit per invocation. There is no `--all`: three stale units is
    /// three deliberate commands. Local only — which image a process is
    /// executing is readable only on the machine holding that process.
    Refresh(ServiceImageRefreshArgs),
}

/// One unit whose image `service image refresh` puts back on its declared file.
#[derive(Args)]
pub struct ServiceImageRefreshArgs {
    /// The host's own name for the unit: the launchd label.
    pub name: String,
    /// Succeed without restarting when kernel image identity already matches.
    #[arg(long)]
    pub if_needed: bool,
    #[arg(long)]
    pub json: bool,
}
