//! Reading a host and changing its machine-level state: beacons, accounts,
//! board power, reachability and unit logs. The first block of
//! `stado host --help`.

use clap::Subcommand;

use super::users::HostUserCommands;

/// The first block of `stado host` verbs. Flattened into
/// `super::HostCommands`, so splitting the declaration across files changes
/// no command line.
#[derive(Subcommand)]
pub(crate) enum HostStateCommands {
    /// Show the latest Stado health beacon and log tail for TARGET.
    Health {
        target: String,
        /// Emit the beacon and object metadata as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Publish one locally collected beacon through the scoped Stado health API.
    ///
    /// The `link` block (tailnet path, sleep/wake, interface changes) is
    /// collected here, on the host, and merged into the document about this
    /// machine before it is published.
    #[command(name = "publish-beacon")]
    PublishBeacon {
        /// JSON beacon file, or '-' for stdin.
        source: String,
        /// Print the document that would be published; publish nothing.
        #[arg(long)]
        print: bool,
    },
    /// The unit ids the registry declares for THIS host, one per line.
    ///
    /// What the health beacon must ask about. The collector's list was an
    /// operator-typed `WC_HEALTH_UNITS`, so a service the registry declared
    /// and the beacon never watched read as a unit that does not exist:
    /// `registry doctor` reported `missing-plist` for
    /// `com.wisent.compute.service.stado-resolver.service.service` on
    /// ubuntu-server-rtx-pro-6000 while that unit was active with a live pid.
    ///
    /// Prints nothing and succeeds when this machine is not in the registry or
    /// the registry cannot be read: a beacon that fails to collect reports
    /// nothing at all, which is worse than reporting the operator's own list.
    #[command(name = "beacon-units")]
    BeaconUnits,
    /// Request a graceful reboot of TARGET through its approved channel.
    Reboot { target: String },
    /// Manage local macOS and Linux user accounts.
    #[command(subcommand)]
    User(HostUserCommands),
    /// Persist and immediately reconcile TARGET's NVIDIA board power cap.
    #[command(name = "gpu-power-limit")]
    GpuPowerLimit {
        target: String,
        watts: u32,
        /// Emit the registry generation and driver report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Report TARGET's uptime, load averages and logged-in users.
    Uptime {
        target: String,
        /// Emit the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Check TARGET's ssh reachability AND health-beacon age as one verdict.
    Ping {
        target: String,
        /// Emit the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Why HOST is claiming nothing: its own agent's published gates, the
    /// disk policy behind them, and what it declared against what it has.
    ///
    /// Read-only and safe against a live host. The Mac mini claimed nothing
    /// for hours at 2 GiB free against a 55 GiB policy, publishing
    /// `disk_pressure_unresolved` every tick, and no command said so.
    Gates {
        host: String,
        /// Emit the gates as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Why TARGET went quiet: beacon age, the path and endpoint it published,
    /// its last sleep and wake, its interface changes, the silences recorded
    /// against it, and what readers refused because of them.
    ///
    /// Read-only and safe against a live host. control-host was
    /// unreachable from 18:29 to 18:35 UTC on 2026-08-19 and came back on a
    /// direct path; nothing in this product carried a trace of it.
    Link {
        target: String,
        /// Emit the link report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// The tail of one managed unit's own log on TARGET.
    ///
    /// A crash-looping unit says why in its log and nowhere else: the health
    /// beacon reports it failed and carries no log, and `host exec` is a
    /// read-only allowlist that cannot read a file.
    #[command(name = "unit-log")]
    UnitLog {
        target: String,
        /// Unit label as launchd knows it, e.g. com.wisent.always-on.brama.
        unit: String,
        /// Tail this many lines from each declared log path (default 40).
        #[arg(long)]
        lines: Option<u32>,
        #[arg(long)]
        json: bool,
    },
    #[command(name = "storage-root-reconcile-worker", hide = true)]
    StorageRootReconcileWorker {
        target: String,
        #[arg(long)]
        target_config: String,
        #[arg(long)]
        transaction: String,
        #[arg(long, value_parser = ["run", "resume", "rollback", "finalize"])]
        phase: String,
        #[arg(long)]
        source_revision: String,
        #[arg(long)]
        tool_sha256: String,
        #[arg(long)]
        runner_gate: String,
    },
}
