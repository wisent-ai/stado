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
    /// Collect THIS host's health beacon from the registry's declarations and
    /// the init system's own answers.
    ///
    /// One unit per identity the registry declares here, with the state
    /// launchd or systemd reports for it: `active`, `failed`, `inactive` when
    /// every domain answered that nothing is there, and `unreadable` with the
    /// cause when a domain refused the read or the read failed. A refused
    /// read is never published as `inactive` — the collector that did that
    /// published a loaded gateway as not loaded.
    ///
    /// Prints the document and publishes nothing unless `--publish` is given.
    #[command(name = "collect-beacon")]
    CollectBeacon {
        /// Publish the collected document through the scoped health API.
        #[arg(long)]
        publish: bool,
    },
    /// What macOS lets TARGET's Stado process read: Documents, Desktop and
    /// Downloads, each `granted`, `denied`, `absent` or `unreadable`, beside
    /// the grants `targets[].privacy_grants` declares for TARGET.
    ///
    /// Read from TARGET's latest beacon, which the host process measures
    /// itself. Exits non-zero when a declared grant is denied, naming the
    /// program, the folder, the declared reason and the System Settings pane
    /// that allows it; a folder no grant declares is reported and fails
    /// nothing.
    Privacy {
        target: String,
        /// Emit the measurement as JSON.
        #[arg(long)]
        json: bool,
        /// Open System Settings → Privacy & Security → Files and Folders;
        /// only for the machine running this command.
        #[arg(long)]
        open: bool,
    },
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
    /// Withdraw TARGET's declared board power cap and return every GPU to
    /// the driver's default limit; the agent stops re-asserting a cap.
    #[command(name = "gpu-power-limit-unset")]
    GpuPowerLimitUnset {
        target: String,
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
    /// Report HOST's measured disk space against the disk-full rule, the
    /// published admission decision and the source, duration and error of
    /// every diagnostic read.
    ///
    /// Read-only. Each registry, host, storage, capacity and queue read
    /// retains its result or concrete error. Elapsed time is recorded, not
    /// used to end a read. Completed readings survive failures; incomplete
    /// reports use claiming=null.
    Gates {
        host: String,
        /// Emit the gates as JSON.
        #[arg(long)]
        json: bool,
        /// Exit on whether the volume is under the disk-full threshold
        /// instead of on `claiming`.
        #[arg(long)]
        require_disk: bool,
    },
    /// Why TARGET went quiet: beacon age, the path and endpoint it published,
    /// its last sleep and wake, its interface changes, the silences recorded
    /// against it, and what readers refused because of them.
    ///
    /// Read-only and safe against a live host. A control host can be
    /// unreachable for minutes and come back on a direct path with nothing in
    /// this product carrying a trace of it; this is that trace.
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
        /// Unit label as launchd knows it, e.g. com.wisent.brama.
        unit: String,
        /// Tail this many lines from each declared log path.
        #[arg(long)]
        lines: u32,
        #[arg(long)]
        json: bool,
    },
    /// Which process holds one TCP port on TARGET: its pid, user and command.
    ///
    /// A unit that exits with `Address already in use` names no owner, and
    /// `service reap` keeps a row a declared label holds, so the socket's
    /// holder was the one fault on this fleet nothing in the product could
    /// name.
    #[command(name = "port-owner")]
    PortOwner {
        target: String,
        /// The TCP port to look up.
        #[arg(long)]
        port: u32,
        #[arg(long)]
        json: bool,
    },
    /// Run PROGRAM on this host under an exclusive, non-blocking flock of
    /// LOCK; the fixed recovery programs' lock holder.
    #[command(name = "run-locked", hide = true)]
    RunLocked {
        lock: String,
        #[arg(last = true)]
        program: Vec<String>,
    },
    /// The host half of the object-API recovery: its readers of this host's
    /// config, launchd definitions, route and Skarbiec release state.
    #[command(name = "object-api-local", hide = true, subcommand)]
    ObjectApiLocal(crate::cli::host::ObjectApiLocalCommands),
    /// Return one release-catalog coordinate in this host's local stores to
    /// the managed account; the host half of the release-store repair.
    #[command(name = "release-store-repair-local", hide = true)]
    ReleaseStoreRepairLocal {
        #[arg(long)]
        config: String,
        #[arg(long)]
        product: String,
    },
    /// The host half of `stado host backup-audit`: classify this host's
    /// replica against its primary store and, with --reclaim yes --apply yes,
    /// delete the twins this pass proved. Prints the marker lines the
    /// operator side reads.
    #[command(name = "backup-audit-local", hide = true)]
    BackupAuditLocal {
        #[arg(long)]
        backup: std::path::PathBuf,
        #[arg(long)]
        primary: std::path::PathBuf,
        #[arg(long)]
        namespace: String,
        #[arg(long, value_parser = ["yes", "no"])]
        reclaim: String,
        #[arg(long, value_parser = ["yes", "no"])]
        apply: String,
        /// Exact object paths, each hex-encoded, comma separated
        #[arg(long, default_value = "")]
        objects_hex: String,
        /// Namespaces to inventory, each hex-encoded, comma separated
        #[arg(long, default_value = "")]
        inventory_namespaces_hex: String,
    },
    /// The host half of a port nobody chooses: binds a free loopback port the
    /// host's own system hands out, releases it and prints its number, so
    /// `service ensure` records a catalog service's port and `service
    /// directory consumer-add` a resolver adapter's in the registry.
    #[command(name = "free-port-local", hide = true)]
    FreePortLocal,
    /// The host half of `stado host storage-root-reconcile`: one phase on
    /// this host's two local storage roots. Prints the marker line the
    /// transaction worker reads; the owner token and the inherited lock
    /// descriptor arrive in STADO_RECONCILE_OWNER_TOKEN and
    /// STADO_RECONCILE_LOCK_FD.
    #[command(name = "storage-root-reconcile-local", hide = true)]
    StorageRootReconcileLocal {
        #[arg(long)]
        phase: String,
        #[arg(long)]
        transaction: String,
    },
    /// The host steps of `stado host storage-root-reconcile` outside its
    /// receipt phases: launching the resident worker, unit files, listener
    /// and object API observations.
    #[command(name = "storage-root-reconcile-host", hide = true, subcommand)]
    StorageRootReconcileHost(crate::deploy::host_storage_reconcile_host::HostCommands),
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
