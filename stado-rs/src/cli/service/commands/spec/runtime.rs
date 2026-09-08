//! The runtime verbs of `stado service`: image refresh, restart, update,
//! release, and the file and secret deliveries.

use clap::Subcommand;

/// The second block of `stado service` verbs. Flattened into
/// [`super::super::ServiceCommands`], so splitting the declaration across
/// files changes no command line.
#[derive(Subcommand)]
pub enum RuntimeCommands {
    /// Put one unit back on the file its `ProgramArguments` name, and prove
    /// it landed.
    ///
    /// The verb behind `registry doctor`'s `stale-unit-image` row. It refuses
    /// a unit that is not stale, naming the identity it found, because a
    /// command that restarts whatever it is pointed at is a restart button.
    /// It re-reads the image afterwards and exits non-zero if the restart did
    /// not change it: launchd re-execs the declared path, and on 2026-09-03
    /// pid 49727 respawned under `KeepAlive` straight back onto the same
    /// unlinked inode it had just left.
    ///
    /// One unit per invocation. There is no `--all`: three stale units is
    /// three deliberate commands. Local only — which image a process is
    /// executing is readable only on the machine holding that process.
    RefreshImage {
        /// The host's own name for the unit: the launchd label.
        name: String,
        /// Succeed without restarting when kernel image identity already matches.
        #[arg(long)]
        if_needed: bool,
        #[arg(long)]
        json: bool,
    },

    /// Restore upstream signed macOS GitHub runner apphosts without restarting.
    RepairRunnerRuntime {
        /// An adopted service that directly launches GitHub's runsvc.sh.
        name: String,
        #[arg(long)]
        host: String,
        #[arg(long)]
        json: bool,
    },

    /// Restart one managed unit, without a full host-recovery pass.
    Restart {
        /// Service name, or the host's own name for the unit.
        name: String,
        /// Restrict to one registry host; omit to restart it everywhere it
        /// is managed.
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
        #[arg(long)]
        readiness_url: Option<String>,
        /// Maximum seconds to wait for readiness.
        #[arg(long, default_value_t = 30)]
        readiness_timeout_seconds: u64,
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

    /// Synchronize one Skarbiec field into a service's runtime env file.
    ///
    /// The value is read through the isolated service-verifier grant and carried
    /// in the SSH request body. It is never printed or placed in argv.
    SecretSync {
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

    /// Synchronize one local file into a managed service's target home.
    ///
    /// The content travels only inside the approved encrypted channel's
    /// request body. It is never printed or placed in an argument vector, and
    /// the destination is replaced atomically with owner-only permissions.
    FileSync {
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
    /// The opposite direction of `file-sync`, and the byte-exact counterpart
    /// of `env-show`. `env-show` sanitizes every value it reports — printable
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
    /// non-zero. `$HOME` confinement and symlink refusal are `env-show`'s,
    /// word for word.
    FileFetch {
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
