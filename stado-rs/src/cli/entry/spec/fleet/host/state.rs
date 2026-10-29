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
    /// The health beacon each host publishes about itself: list every
    /// host's, collect this host's, or publish one collected elsewhere.
    #[command(subcommand)]
    Beacon(HostBeaconCommands),
    /// What macOS lets TARGET's Stado process read of the folders
    /// `targets[].privacy_grants` declares for it — Documents, Desktop and
    /// Downloads, each `granted`, `denied`, `absent`, `unreadable` or `not
    /// measured` (a folder no grant declares for the program is never opened)
    /// — and, with `--since`, every privacy request macOS recorded against
    /// that program in the window.
    ///
    /// The folders are read from TARGET's latest beacon, which the host
    /// process measures itself; the requests from TARGET's unified log,
    /// through its declared connection. Exits non-zero when a declared grant
    /// is denied, naming the program, the folder, the declared reason and the
    /// System Settings pane that allows it, and when a request reached for
    /// protected data no grant declares, naming its service and time.
    Privacy {
        target: String,
        /// Emit the measurement as JSON.
        #[arg(long)]
        json: bool,
        /// Open System Settings → Privacy & Security → Files and Folders;
        /// only for the machine running this command.
        #[arg(long)]
        open: bool,
        /// Also list the privacy requests TARGET's unified log recorded
        /// against its Stado program in this window: a whole number and m, h
        /// or d (`1d` is the last day).
        #[arg(long, value_name = "WINDOW")]
        since: Option<String>,
    },
    /// Whether anybody can answer a question macOS shows at TARGET's screen:
    /// who owns /dev/console and which display panels (the built-in one, or
    /// a monitor that names itself) macOS reports online, measured on
    /// the host and recorded as `targets[].console` (`headless` or
    /// `attended`).
    ///
    /// On a Mac whose recorded console is headless, Stado's units run as
    /// launchd daemons, which macOS never asks about the local network, and
    /// `stado host upgrade` plans no dialog there.
    #[command(subcommand)]
    Console(HostConsoleCommands),
    /// Request a graceful reboot of TARGET through its approved channel.
    Reboot { target: String },
    /// Manage local macOS and Linux user accounts.
    #[command(subcommand)]
    User(HostUserCommands),
    /// TARGET's GPUs: the declared NVIDIA board power cap, set or unset.
    #[command(subcommand)]
    Gpu(HostGpuCommands),
    /// Report TARGET's uptime, load averages and logged-in users.
    Uptime {
        target: String,
        /// Emit the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// The build caches (directories a CACHEDIR.TAG marks) in TARGET's home
    /// that Spotlight still indexes; --apply writes the .metadata_never_index
    /// that keeps Spotlight out of each.
    Spotlight {
        target: String,
        /// Write the marker into every unmarked cache.
        #[arg(long)]
        apply: bool,
        /// Emit the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// What each process on TARGET spent since the reading before: CPU
    /// seconds (its own and its reaped children's), resident memory and bytes
    /// read from and written to storage, read from the kernel by the host's
    /// own Stado. The first reading on a host says it has nothing to compare
    /// with yet.
    Processes {
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
    /// TARGET's TCP ports: which process holds one.
    #[command(subcommand)]
    Port(HostPortCommands),
    /// The halves of host verbs that run on this host itself, started over
    /// the host channel with this host's installed Stado: a lock to run
    /// under, the object API's readers, the release store's repair, the
    /// backup's audit, a port the host hands out, and the storage-root
    /// transaction's phases, steps and worker.
    #[command(subcommand)]
    Local(super::local::HostLocalCommands),
}

/// TARGET's console, measured on the host and recorded in the registry.
#[derive(Subcommand)]
pub(crate) enum HostConsoleCommands {
    /// Measure TARGET's console and print it beside the recorded one.
    /// Exits non-zero when they differ, or when TARGET measures headless and
    /// records no console.
    Show {
        target: String,
        /// Emit the measurement, the record and the verdict as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Measure TARGET's console and record the measurement as
    /// `targets[].console`. A headless console is refused while the target
    /// declares a privacy grant or a work root in Documents, Desktop or
    /// Downloads, each a question nobody there answers.
    Record {
        target: String,
        /// Emit the measurement and the registry generation as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Remove TARGET's recorded console; Stado then treats the host as
    /// attended.
    Unset {
        target: String,
        /// Emit whether a record was removed and the registry generation as
        /// JSON.
        #[arg(long)]
        json: bool,
    },
}

/// TARGET's GPUs, as the fleet declares and reads them.
#[derive(Subcommand)]
pub(crate) enum HostGpuCommands {
    /// The board power cap a local target declares in the registry
    /// (`gpu_power_limit_watts`), which its agent re-asserts every poll.
    #[command(subcommand)]
    Power(GpuPowerLimitCommands),
}

/// The board power cap a local target declares in the registry
/// (`gpu_power_limit_watts`), which its agent re-asserts every poll.
#[derive(Subcommand)]
pub(crate) enum GpuPowerLimitCommands {
    /// Persist and immediately reconcile TARGET's NVIDIA board power cap.
    Set {
        target: String,
        watts: u32,
        /// Emit the registry generation and driver report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Withdraw TARGET's declared board power cap and return every GPU to
    /// the driver's default limit; the agent stops re-asserting a cap.
    Unset {
        target: String,
        /// Emit the registry generation and driver report as JSON.
        #[arg(long)]
        json: bool,
    },
}

/// TARGET's TCP ports.
#[derive(Subcommand)]
pub(crate) enum HostPortCommands {
    /// Which process holds one TCP port on TARGET: its pid, user and command.
    ///
    /// A unit that exits with `Address already in use` names no owner, and
    /// `service reap` keeps a row a declared label holds, so the socket's
    /// holder was the one fault on this fleet nothing in the product could
    /// name.
    Owner {
        target: String,
        /// The TCP port to look up.
        #[arg(long)]
        port: u32,
        #[arg(long)]
        json: bool,
    },
}

/// The health beacon: one document per host, published on a timer, that every
/// liveness verdict in Stado reads.
#[derive(Subcommand)]
pub(crate) enum HostBeaconCommands {
    /// Every registry host and its last beacon, worst first.
    List {
        /// Emit the table as JSON.
        #[arg(long)]
        json: bool,
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
    Collect {
        /// Publish the collected document through the scoped health API.
        #[arg(long)]
        publish: bool,
    },
    /// Publish one locally collected beacon through the scoped Stado health API.
    ///
    /// The `link` block (tailnet path, sleep/wake, interface changes) is
    /// collected here, on the host, and merged into the document about this
    /// machine before it is published.
    Publish {
        /// JSON beacon file, or '-' for stdin.
        source: String,
        /// Print the document that would be published; publish nothing.
        #[arg(long)]
        print: bool,
    },
}
