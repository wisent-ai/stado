//! The reads and host inspections of `stado service`, in the order
//! `--help` prints them.

use clap::Subcommand;

/// The first block of `stado service` verbs. Flattened into
/// [`super::super::ServiceCommands`], so splitting the declaration across
/// files changes no command line.
#[derive(Subcommand)]
pub enum ReadCommands {
    /// Where a service is reachable from here, and who may use it.
    #[command(subcommand)]
    Directory(crate::cli::directory::DirectoryCommands),

    /// The preconfigured Wisent services, ready to deploy by name: no
    /// declaration to write, no flags to know. `service deploy <name>` and
    /// `service ensure <name>` resolve these when nothing else declares the
    /// unit.
    Catalog {
        #[arg(long)]
        json: bool,
    },

    /// Every registry-managed service across all hosts, with its state.
    ///
    /// Answered from the latest health beacons, so it costs no ssh and
    /// reports on hosts that are not currently reachable. A host that has
    /// published no beacon reports `unknown`, which is deliberately not
    /// the same answer as `missing`.
    ///
    /// `OBSERVED` is a different question from `STATE` and is answered by a
    /// different party. `STATE` is what the host says about its own unit;
    /// `OBSERVED` is when anybody last went and looked at the service from
    /// outside. A host with a closed lid publishes no beacon and says
    /// nothing, so `STATE` goes quiet rather than wrong -- and quiet is what
    /// read as fine for twelve days. `never` in this column means no machine
    /// has ever confirmed this service from any vantage.
    ///
    /// `--unowned` answers the opposite question: which product processes are
    /// running that no launchd job or systemd unit owns. Two `stado agent`
    /// processes ran that way for four days, executing a binary older than the
    /// one on disk, and every answer in this group was about declared units
    /// and so said nothing about them.
    ///
    /// `--undeclared` answers the third question, which had no answer at all:
    /// which units launchd has LOADED that the registry does not declare.
    /// Neither of the other two can see one — `list` walks the document and
    /// asks the host about each entry, `--unowned` walks the processes and asks
    /// launchd who owns them, and a loaded job the document never heard of is
    /// in neither set. charless-mac-mini ran three queue agents at once in that
    /// blind spot for seven days.
    List {
        /// Report the product processes no unit owns instead of the declared
        /// managed set. This is the one question in this group the beacons
        /// cannot answer -- an unowned process is by definition in nobody's
        /// declaration -- so it costs one read-only ssh per kind=local host.
        #[arg(long)]
        unowned: bool,
        /// Report the launchd jobs a host has loaded under this fleet's own
        /// label prefix that the registry does not declare. One read-only
        /// `launchctl list` per kind=local host.
        #[arg(long)]
        undeclared: bool,
        #[arg(long)]
        json: bool,
    },

    /// Boot one exact launchd label or systemd unit out of its system or user
    /// scope.
    ///
    /// `stop` and `retire` require a registry declaration. This command is for
    /// a loaded unit the registry does not declare, including an obsolete
    /// duplicate that `service list --undeclared` found. It never removes the
    /// unit's file.
    ///
    /// On Linux the selected systemd manager stops and disables only the exact
    /// requested unit, then Stado reads back that it is inactive and not
    /// enabled. On Darwin the selected launchd domain is booted out as before.
    Bootout {
        /// Exact launchd label or systemd unit name, as the host knows it.
        label: String,
        /// Registry host that has it loaded.
        #[arg(long)]
        host: String,
        /// Which init-system scope to act in: `system`, `user`, or unset for
        /// `any`. The unset order is system first, then the calling account
        /// only when the system scope holds no exact unit by this name. A name
        /// loaded in both scopes identifies two jobs; pass `user` to leave its
        /// system sibling untouched.
        #[arg(long)]
        domain: Option<String>,
        #[arg(long)]
        json: bool,
    },

    Reap {
        /// Registry host to reap. Required: this signals processes.
        #[arg(long)]
        host: String,
        /// The exact program being de-duplicated, as a substring of its command
        /// line -- for example `stado agent --target charless-mac-mini`.
        /// Required, and deliberately not defaulted: a fleet-wide reap on that
        /// host proposed ending `skarbiec serve`, `stado dashboard`,
        /// `stado resolver serve` and the Weles API server, because launchd
        /// holds a pid for only some declared labels and everything else read
        /// as undeclared.
        #[arg(long)]
        command: String,
        /// Send SIGTERM to the rows a declared label does not hold. Without it
        /// those rows read `would_end`; a `kept` row is never signalled with
        /// or without this flag, and is reported so the program a declared
        /// label is running can be named.
        #[arg(long)]
        apply: bool,
        #[arg(long)]
        json: bool,
    },

    /// Sit on HOST and name the parent of the next process matching a
    /// program, while that parent is still alive.
    ///
    /// `reap` and `list --unowned` each take one snapshot, and a snapshot
    /// taken after a respawn can only ever report `ppid 1` — the parent
    /// backgrounded the child and exited, which is precisely why nothing
    /// could say what kept restarting an undeclared `stado agent` on
    /// charless-mac-mini. Driving a snapshot from here in a loop cannot
    /// sample faster than an SSH round trip; the loop has to run on the host.
    ///
    /// Reads `ps` on an interval and prints. It signals nothing, starts
    /// nothing and writes nothing, so it is safe to leave running while
    /// somebody else works on the box.
    #[command(name = "watch-spawn")]
    WatchSpawn {
        /// Registry host to watch.
        #[arg(long)]
        host: String,
        /// The program to watch for, as a substring of its command line --
        /// for example `stado agent --target charless-mac-mini`. Processes
        /// matching it that are ALREADY running when the watch opens are
        /// reported as baseline and never as arrivals.
        #[arg(long)]
        command: String,
        /// How long to watch, in seconds.
        #[arg(long, default_value_t = 300)]
        seconds: u64,
        /// Gap between samples, in milliseconds. The default catches a parent
        /// that lives about a second; tighten it for one that does not.
        #[arg(long, default_value_t = 1000)]
        interval_ms: u64,
        #[arg(long)]
        json: bool,
    },

    /// Ask the host init system what it holds under one named unit.
    ///
    /// This reader does not enumerate. The operator names the launchd label or
    /// systemd unit, so it can inspect a loaded unit whose file is gone. Fixed
    /// process, path, restart and trigger fields plus the five explicitly
    /// non-secret storage-routing variables are returned; no other service
    /// environment is read.
    #[command(name = "label-print")]
    LabelPrint {
        /// launchd label or systemd unit, as the host knows it.
        label: String,
        /// Registry host to ask.
        #[arg(long)]
        host: String,
        /// Init-system domain: `system`, `user`, or unset to ask both.
        #[arg(long)]
        domain: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// Go to each consumer and check the endpoint it is told to use, and that
    /// the thing answering is the service that was declared.
    ///
    /// `list` reports what hosts say about their units. This reports whether
    /// the directory's addresses answer, from the machines that must call
    /// them -- the one question every other check in this binary skips.
    /// States are `observed`, `unreachable`, `misowned` for a port a different
    /// declared unit is holding, and `unverified` for a probe that could not
    /// run; the last is never folded into the others.
    ///
    /// `misowned` exists because an answer was once the whole of `observed`'s
    /// evidence. On the service's active host the port's owner is resolved by
    /// launchd label through the same reader `service serving` uses, so a
    /// declaration pointing at a port another job holds is a failure here
    /// rather than a green row. Other hosts reach the service through their
    /// own resolver adapter and are not judged on ownership, because that
    /// socket is owned by the resolver by design.
    ///
    /// Exits non-zero on `unreachable` and `misowned`, counted separately: the
    /// first usually means the service needs attention, the second means the
    /// declaration does.
    Verify {
        /// Check one host's declarations instead of the whole fleet.
        #[arg(long)]
        host: Option<String>,
        /// Probe from this machine only, without using the fleet channel.
        /// This is the mode the installed probe helper runs.
        #[arg(long)]
        local: bool,
        #[arg(long)]
        json: bool,
    },

    /// Is the host running the version the registry declares for it?
    ///
    /// `list` and `show` answer questions about the unit -- loaded, running,
    /// which program -- and every one of those answers stays true across a
    /// release that never reached the box. This compares
    /// `targets[].managed_versions`, the declared version of each managed
    /// binary on TARGET, against the version that host actually runs.
    ///
    /// Verdicts are `in-sync`, `drifted`, and `unknown` for a binary whose
    /// installed version could not be read; the third is never folded into
    /// either of the other two. Reporting exits non-zero on `drifted` alone,
    /// so an uninstalled reporting helper cannot masquerade as drift.
    /// `--apply` delivers the declared version of every drifted binary through
    /// `stado host release`, re-reads the installed versions afterwards, and
    /// exits non-zero unless every binary in scope is confirmed `in-sync`.
    Converge {
        /// Registry host to compare against its own declarations.
        target: String,
        /// One managed binary by name; omit for every binary TARGET declares
        /// a version for.
        binary: Option<String>,
        /// Deliver the declared version of every drifted binary, then read the
        /// installed versions back.
        #[arg(long)]
        apply: bool,
        #[arg(long)]
        json: bool,
    },

    /// Registry-managed services carrying Echo onboarding product metadata.
    ///
    /// Emits the versioned JSON envelope accepted by Echo's Stado catalog
    /// synchronization endpoint.
    OnboardingCatalog,

    /// One service's state everywhere it is managed.
    Status {
        /// Service name, or the host's own name for the unit.
        name: String,
        #[arg(long)]
        json: bool,
    },
}
