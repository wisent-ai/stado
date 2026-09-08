//! The lifecycle verbs of `stado service`, plus its log and env reads.

use clap::Subcommand;

/// The last block of `stado service` verbs. Flattened into
/// [`super::super::ServiceCommands`], so splitting the declaration across
/// files changes no command line.
#[derive(Subcommand)]
pub enum LifecycleCommands {
    /// Bring an existing launchd/systemd unit under management.
    ///
    /// The unit must already exist on the host — adoption claims what is
    /// there, it does not create anything. The host is probed first and the
    /// registry records what the host reported, not what was assumed.
    Adopt {
        /// launchd label or systemd unit name, as the host knows it.
        unit: String,
        /// Explicit registry host that runs it.
        #[arg(
            long,
            conflicts_with = "host_heuristic",
            required_unless_present = "host_heuristic"
        )]
        host: Option<String>,
        /// Declarative placement selector resolved against the registry.
        #[arg(long, conflicts_with = "host")]
        host_heuristic: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// Attach central onboarding product metadata to a managed service.
    ///
    /// The metadata becomes part of the canonical Stado registry and is
    /// emitted by `service list --json` for Echo catalog synchronization.
    Onboarding {
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
        #[arg(long, value_delimiter = ',', num_args = 1..)]
        surfaces: Vec<String>,
        #[arg(long)]
        first_success_fact: String,
        #[arg(long, default_value = "both")]
        onboarding_kind: String,
        #[arg(long, default_value = "active")]
        status: String,
        #[arg(long)]
        json: bool,
    },

    /// Remove a service from management: bootout/disable and forget.
    ///
    /// Unit files are left on disk. Retiring is a management decision, not
    /// a deletion.
    Retire {
        /// launchd label or systemd unit name, as the host knows it.
        unit: String,
        /// Registry host that runs it.
        #[arg(long)]
        host: String,
        #[arg(long)]
        json: bool,
    },

    /// Hand a placed logical service's lifecycle to its active signed release.
    ///
    /// The release must already be committed and ready, and the legacy unit
    /// must already be inactive. One conditional registry write then removes
    /// every legacy restart identity while preserving the logical route,
    /// placement dependency, probes, and release policy.
    ///
    /// Repeating an incomplete handoff rechecks the same release, rollout
    /// generation and exact legacy files before committing against the current
    /// registry. The prior commit remains in the receipt's recovery history.
    /// A completed receipt cannot be reapplied against a mismatching registry.
    HandoffReleaseControl {
        /// Logical service in the service directory and placement profile.
        service: String,
        /// Active registry host serving the release-controlled stable bind.
        #[arg(long)]
        host: String,
        /// Exact product in registry.release_control.
        #[arg(long)]
        product: String,
        #[arg(long)]
        json: bool,
    },

    /// Remove a service entirely: withdraw its declaration, stop it, and
    /// delete its unit file from the host — the operation an operator means
    /// by "remove this service", which `retire` deliberately is not. The file
    /// path comes from the registry declaration, never from operator words.
    /// A host failure restores the declaration; a file-delete failure leaves
    /// the service retired and reports that partial state.
    Remove {
        /// launchd label or systemd unit name, as the host knows it.
        unit: String,
        /// Registry host that runs it.
        #[arg(long)]
        host: String,
        #[arg(long)]
        json: bool,
    },

    /// Install a new unit under management: render, push, bootstrap,
    /// record.
    Deploy {
        /// Service name; lowercase letters, digits, '.', '-' and '_'.
        name: String,
        /// Explicit registry host to install it on.
        #[arg(
            long,
            conflicts_with = "host_heuristic",
            required_unless_present = "host_heuristic"
        )]
        host: Option<String>,
        /// Declarative placement selector resolved against the registry.
        #[arg(long, conflicts_with = "host")]
        host_heuristic: Option<String>,
        /// Absolute path, ON THE TARGET HOST, of the program the unit runs.
        /// The plist / systemd unit is rendered around it by the same
        /// renderer `stado bootstrap --local` uses.
        #[arg(long, conflicts_with = "from_artifact")]
        from: Option<String>,
        /// Published artifact to install and run instead of a path already on
        /// the host. The reference is resolved to an immutable version, that
        /// version is placed under ~/.stado/services/NAME/<version>/, its
        /// declared sha256 is verified there, and `current` is moved onto it.
        /// The unit runs through `current`, so a later install or a rollback
        /// is a relink rather than a redeploy.
        #[arg(long = "from-artifact")]
        from_artifact: Option<String>,
        /// One argument the unit is started with; repeat for each. A program
        /// that needs a subcommand or a port to be the service it is named
        /// after cannot be deployed without these, and hand-starting it
        /// beside the unit is how a host ends up serving on a port no
        /// declaration mentions.
        #[arg(long = "arg")]
        args: Vec<String>,
        /// Keep this exact launchd label instead of minting one from NAME.
        /// Darwin only; used when a managed daemon is recreated as a
        /// per-login LaunchAgent without changing its service identity.
        #[arg(long = "launchd-label")]
        launchd_label: Option<String>,
        /// Install a Darwin service as a per-login LaunchAgent even when the
        /// target is declared always-on. The host must have a live gui/<uid>
        /// domain; deployment refuses instead of falling back to a daemon.
        #[arg(long = "as-launch-agent")]
        as_launch_agent: bool,
        #[arg(long)]
        json: bool,
    },

    /// Declare a service against the fleet's one contract.
    ///
    /// Stado ships no list of services: a service is whatever its author
    /// declares — an immutable source the bytes come from, a run spec the
    /// unit is rendered from, how the service is observed, and who may call
    /// it. This command writes that declaration into the service directory;
    /// `deploy` then needs no flags beyond the name, because everything it
    /// would ask for is already written down.
    Declare {
        /// Path to the declaration file (JSON). Required keys: `name`,
        /// `host`, `source.artifact`, `source.sha256`. Optional: `run`,
        /// `verify`, `consumers`, `endpoints`, or `port` as a shorthand for
        /// one loopback endpoint on the declared host.
        #[arg(long)]
        file: String,
        #[arg(long)]
        json: bool,
    },

    /// Assert the unit a host must be running, over ssh, idempotently.
    ///
    /// `deploy` installs a unit and refuses one that is already declared, so
    /// there was no command an operator could run twice, or run from a script,
    /// to make a host run what it is supposed to run. This one reads what is
    /// there first: a unit already running the declared program is reported
    /// `already_correct` with nothing touched, a unit that exists but is not
    /// running is kicked in place, and a host with no unit gets one.
    ///
    /// It also works where `deploy` cannot. An ssh login has no Aqua session,
    /// `launchctl bootstrap gui/$uid` answers `Could not switch to audit
    /// session ... Operation not permitted`, and `deploy` returned that having
    /// installed nothing — which is how two `stado agent` processes came to run
    /// for four days with no unit behind them. Where the per-login domain does
    /// not exist, the unit is rendered for launchd's system domain and
    /// installed as a daemon in /Library/LaunchDaemons.
    ///
    /// An existing matching definition is restarted in place with
    /// `kickstart -k`. When launchd's retained Program or ProgramArguments
    /// differs from the desired definition, ensure first validates the
    /// replacement executable and complete rendered plist, then reloads that
    /// definition once and verifies launchd's readback and running executable.
    /// An unreadable retained definition is refused without touching the job.
    Ensure {
        /// Service name; lowercase letters, digits, '.', '-' and '_'.
        name: String,
        /// The single registry host that must be running it.
        #[arg(long)]
        host: String,
        /// Absolute path, ON THE TARGET HOST, of the program the unit runs.
        /// Omit it to render the unit from the service's own declaration,
        /// which is what makes a declared service reinstallable from the
        /// document instead of from a plist somebody installed by hand.
        #[arg(long)]
        from: Option<String>,
        /// One argument the unit is started with; repeat for each. Only with
        /// `--from`: the declared argument vector belongs to the declared
        /// program and the two are never mixed.
        #[arg(long = "arg")]
        args: Vec<String>,
        /// Non-secret NAME=VALUE persisted with the unit; repeat for each key.
        /// Use secret-sync for credentials, never put them on the command line.
        #[arg(long = "env", value_name = "NAME=VALUE")]
        env: Vec<String>,
        /// Why this host must run this unit. Required: `ensure` installs units
        /// and restarts running ones, and every such change is recorded beside
        /// the registry document it declared the unit in.
        #[arg(long)]
        reason: String,
        /// Install the unit as a system LaunchDaemon
        /// (`/Library/LaunchDaemons/<label>.plist`) instead of following the
        /// declaration or the per-login fallback. Implied for a registry host
        /// declared always-on on Darwin, where that is the only domain a
        /// service stays alive in; pass it for a host whose declaration does
        /// not say so yet. The privileged install and bootstrap steps run
        /// under passwordless sudo, and a host without that grant is told
        /// exactly which step was refused.
        #[arg(long, conflicts_with = "as_launch_agent")]
        as_daemon: bool,
        /// Recreate a declared Darwin unit as a per-login Aqua LaunchAgent.
        /// The old unit must be unloaded and its old plist removed first;
        /// ensure then updates the existing registry record in one write.
        #[arg(long = "as-launch-agent", conflicts_with = "as_daemon")]
        as_launch_agent: bool,
        #[arg(long)]
        json: bool,
    },

    /// Tail a managed unit's log over the approved channel.
    Logs {
        /// Service name, or the host's own name for the unit.
        name: String,
        /// Restrict to one registry host; omit to tail every host that
        /// manages it.
        #[arg(long)]
        host: Option<String>,
        /// Lines of tail to fetch.
        #[arg(long, default_value_t = default_log_lines())]
        lines: usize,
        #[arg(long)]
        json: bool,
    },

    /// The effective environment a managed unit runs with, secrets
    /// redacted.
    ///
    /// Parsed from the plist or systemd unit and its drop-ins. Values whose
    /// variable name looks like a credential are replaced, in the table and
    /// in `--json` alike.
    Env {
        /// Service name, or the host's own name for the unit.
        name: String,
        /// Restrict to one registry host; omit to read every host that
        /// manages it.
        #[arg(long)]
        host: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

/// Default `--lines` for `service logs`: one byte's worth of lines. Derived
/// from `u8::MAX` rather than written as a number, the same way
/// `cli/mod.rs::default_mail_results` derives its default from `u8::BITS`.
fn default_log_lines() -> usize {
    usize::from(u8::MAX)
}
