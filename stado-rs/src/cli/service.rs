//! `stado service ...` — the full service-management layer.
//!
//! NO Python original: the Python CLI stops at `host recover`, and that gap
//! is the point. `docs/missing-commands.md` items seven through fourteen
//! were written after a wedged `com.wisent.weles-api` sat unmanaged on a
//! mac mini: the unit existed on the host, Stado did not know about it, and
//! there was no command to list it, restart it or adopt it.
//!
//! The engine is [`crate::deploy::service`]; this module is the operator
//! surface over it. Two properties are worth keeping when editing:
//!
//! - `list` answers from the health beacons alone. No ssh, no per-host
//!   round trip, so the fleet-wide question stays answerable when a host
//!   is the thing that is broken. `status` answers the same way, and adds
//!   best-effort host reads — launchd's last exit status and the stderr
//!   tail — only for units whose beacon state is `failed`; those reads
//!   degrade to a note, never to a failed command.
//! - Registry mutations use
//!   `cli/registry.rs::{commit_document, push_document_if}` — the validated
//!   conditional write path — and never hand-edit the document. `retire` and
//!   `remove` also hold the autonomy reconciler's per-unit lease while they
//!   withdraw the declaration and change the host, so a tick with an older
//!   snapshot cannot start the unit during that transaction.

use clap::Subcommand;
use serde::Deserialize;
use serde_json::{json, Value};
use std::future::Future;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use crate::deploy::service::{
    self, ManagedService, ServiceEnv, ServiceLog, ServiceStatus, UnitDomain, SOURCE_RECOVERY,
    SOURCE_REGISTRY,
};
use crate::deploy::{
    host_channel, host_exec, production_runner, service_env_file, service_file_fetch,
    service_label_print, service_serving, service_spawn_watch, DeployError,
};
use crate::observations;
use crate::queue::JobStorage;
use crate::targets;

use super::{registry, table, CmdError};

#[derive(Subcommand)]
pub enum ServiceCommands {
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
    /// systemd unit, so it can inspect a loaded unit whose file is gone. Only
    /// fixed process, path, restart and trigger fields are returned; service
    /// environments are never read.
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

    /// Replace one key in a managed runtime env file or systemd definition.
    ///
    /// The value is read from an owner-only local file and travels only inside
    /// the approved encrypted channel's request body.
    /// A systemd unit or its own .conf drop-in is updated in place; the manager
    /// reloads changed definitions without restarting the running service.
    EnvSet {
        /// Service whose host-local process reads the environment.
        name: String,
        /// The single registry host to update.
        #[arg(long)]
        host: String,
        /// Exact environment variable name.
        #[arg(long)]
        key: String,
        /// Runtime env file, or this service's exact systemd unit/drop-in path.
        #[arg(long)]
        env_file: String,
        /// Absolute owner-only local file containing the value.
        #[arg(long)]
        value_file: String,
        #[arg(long)]
        json: bool,
    },

    /// Remove one key from a managed env file or systemd definition.
    EnvUnset {
        /// Service whose host-local process reads the environment.
        name: String,
        /// The single registry host to update.
        #[arg(long)]
        host: String,
        /// Exact environment variable name.
        #[arg(long)]
        key: String,
        /// Runtime env file, or this service's exact systemd unit/drop-in path.
        #[arg(long)]
        env_file: String,
        #[arg(long)]
        json: bool,
    },

    /// Read a managed service's owner-controlled env file, duplicates and all.
    ///
    /// The counterpart of `env-set`: same approved encrypted channel, same
    /// `$HOME` confinement, opposite direction. `service env` answers what the
    /// UNIT FILE declares; this answers what the file a launcher `.`-sources
    /// declares, which on this fleet is where the interesting values live.
    ///
    /// Every assignment is listed in FILE ORDER with its line number, and a
    /// key assigned twice is reported twice — `effective` for the last
    /// assignment, `shadowed` for every earlier one — because a sourced file
    /// assigns top to bottom and a later duplicate silently wins.
    ///
    /// A value whose key looks like a credential is withheld, and a URL
    /// carrying userinfo is withheld whatever its key is called. An endpoint,
    /// a port, a flag or a `$REFERENCE` is shown whatever its key is called:
    /// those are what an operator reads this file to verify. The decision is
    /// made ON THE HOST, so a withheld value never crosses the channel.
    EnvShow {
        /// Service whose host-local process reads the environment.
        name: String,
        /// The single registry host to read.
        #[arg(long)]
        host: String,
        /// Environment file on the target, absolute or rooted at $HOME.
        #[arg(long)]
        env_file: String,
        /// Show this one variable's value in full, whatever its name suggests.
        /// The key name travels; no secret is ever placed in a remote command
        /// line either way.
        #[arg(long)]
        reveal: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// Does this unit's env file agree with what is actually listening?
    ///
    /// The endpoint half of `env-show`: every loopback URL or port the file's
    /// effective assignments declare, checked against the host's own socket
    /// table, with the process that holds each port named. `host inventory`
    /// does this for forward markers; this does it for a unit's environment.
    ///
    /// Exits non-zero when a loopback endpoint is declared and nothing is
    /// listening there, and when the check could not be performed at all.
    EndpointCheck {
        /// Service whose host-local process reads the environment.
        name: String,
        /// The single registry host to check.
        #[arg(long)]
        host: String,
        /// Environment file on the target, absolute or rooted at $HOME.
        #[arg(long)]
        env_file: String,
        #[arg(long)]
        json: bool,
    },

    /// Is the DECLARED unit the process on its own port?
    ///
    /// `show` reports what the unit file declares and used to spell that
    /// `runs`; `endpoint-check` reports whether anything answers on a declared
    /// port. Neither asks the one question an outage turns on. On 2026-08-30
    /// `com.wisent.always-on.weles` was reported `runs` while both pids its
    /// last restart produced were already gone and its stderr ended in
    /// `EADDRINUSE 127.0.0.1:58101`: something WAS listening there, and it was
    /// a different launchd job — the undeclared unit the Weles release
    /// deployer bootstraps, running an identical argument vector.
    ///
    /// So ownership here is decided by launchd label, never by argv. The pid
    /// holding each port is walked up its own parent chain until a pid appears
    /// in `launchctl list`, because a launcher script is the job and the
    /// server it starts is the child that holds the socket. A label that
    /// cannot be read — a system LaunchDaemon is invisible to an unprivileged
    /// `launchctl list` — is reported `unknown`, never as "nobody owns it".
    ///
    /// Verdicts are `serving`, `not_serving`, and `unknown` for a question
    /// that could not be answered; the third is never folded into either of
    /// the others. Exits non-zero on anything but `serving`, because a control
    /// plane that cannot tell reported this host healthy for days.
    Serving {
        /// Service name, or the host's own name for the unit.
        name: String,
        /// The single registry host to check.
        #[arg(long)]
        host: String,
        /// One loopback port this unit is supposed to SERVE; repeat for each.
        ///
        /// Deliberately not taken from the unit's env file. That file names
        /// every endpoint the unit touches, and most of them are ports it
        /// CALLS — `STADO_API_URL`, `WC_SKARBIEC_URL` — owned by other
        /// services on purpose. Judging those as "this unit must own it" makes
        /// every healthy dependency a finding, which is how a check stops
        /// being read. `endpoint-check` is the command for dependencies; this
        /// one is about the ports the service itself answers on. Omit these
        /// and the service directory's declared endpoint for this host is
        /// used.
        #[arg(long = "port")]
        ports: Vec<u16>,
        #[arg(long)]
        json: bool,
    },

    /// Reconcile one Skarbiec consumer grant with an existing owner-only token file.
    ///
    /// The bearer never leaves the managed host: its local Skarbiec reads the
    /// raw file and records only its hash while replacing the declared grant.
    GrantSync {
        /// Service whose host-local deployer uses the grant.
        name: String,
        /// The single registry host to update.
        #[arg(long)]
        host: String,
        /// Exact Skarbiec consumer name.
        #[arg(long)]
        consumer: String,
        /// One complete grant capability; repeat for every capability.
        #[arg(long = "capability", required = true)]
        capabilities: Vec<String>,
        /// Existing raw bearer file on the target, absolute or rooted at $HOME.
        #[arg(long)]
        token_file: String,
        /// Authoritative Skarbiec vault on the target, absolute or rooted at $HOME.
        #[arg(long, default_value = "$HOME/.stado/skarbiec.vault.json")]
        vault_file: String,
        /// Lifetime of the replacement grant.
        #[arg(long, default_value_t = 2_592_000)]
        ttl_seconds: u64,
        /// Grant audience; defaults to the consumer.
        #[arg(long)]
        audience: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// Write one Skarbiec item field into an owner-only raw bearer file.
    ///
    /// `WC_STADO_STORAGE_TOKEN_FILE` has to name a file whose entire content
    /// is the bearer, because `queue/stado_object.rs` resolves a token file
    /// and nothing else. `secret-sync` can put a Skarbiec field into a unit's
    /// env file, and `grant-sync` can reconcile a grant against a token file
    /// that is already on the host, but nothing could create that file. So the
    /// only remaining way to bind a host to the fleet object store was to
    /// hand-copy a secret onto it, which is the one thing the fleet-wide
    /// "everything through Stado" rule exists to prevent. Lacking the file,
    /// charless-mac-mini's queue agent bound its `JobStorage` to a
    /// device-local store instead and published no capacity for seven days
    /// while 74 fleet jobs waited on a host every surface reported as
    /// in-sync -- a fleet claim written to a device store does not fail, it
    /// succeeds where nobody else can see it.
    ///
    /// The value is read through the isolated service-verifier grant and
    /// carried in the SSH request body. It is never printed or placed in argv.
    TokenFileSync {
        /// Service name, or the host's own name for the unit.
        name: String,
        /// The single registry host to update.
        #[arg(long)]
        host: String,
        /// Skarbiec item containing the bearer.
        #[arg(long)]
        item: String,
        /// Exact string field in the Skarbiec item.
        #[arg(long, default_value = "token")]
        field: String,
        /// Destination bearer file on the target, absolute or rooted at $HOME.
        #[arg(long)]
        token_file: String,
        #[arg(long)]
        json: bool,
    },

    /// Verify a managed service's bearer against a read-only loopback endpoint.
    ///
    /// With `--repair`, a failed check atomically synchronizes the secret,
    /// restarts the unit, and checks the endpoint once more.
    AuthCheck {
        /// Service name, or the host's own name for the unit.
        name: String,
        /// The single registry host to check.
        #[arg(long)]
        host: String,
        /// Skarbiec item containing the bearer. Required unless the bearer
        /// is read from the unit's own runtime environment with --variable
        /// and --env-file instead.
        #[arg(long)]
        item: Option<String>,
        /// Host-side Skarbiec consumer used to read --item (defaults to the
        /// host's own selection).
        #[arg(long)]
        consumer: Option<String>,
        /// Token file for --consumer.
        #[arg(long)]
        token_file: Option<String>,
        /// Exact string field in the Skarbiec item.
        #[arg(long, default_value = "token")]
        field: String,
        /// Read-only loopback HTTP endpoint that requires authentication.
        #[arg(long)]
        url: String,
        /// Send an empty JSON POST instead of a GET; useful for auth-first APIs.
        #[arg(long)]
        post_empty_json: bool,
        /// Treat this exact HTTP status as proof that authentication passed.
        #[arg(long)]
        expect_status: Option<u16>,
        /// On failure, synchronize the secret, restart, and check again.
        #[arg(long)]
        repair: bool,
        /// If repair still fails, stop the unmanaged process owning the URL port.
        #[arg(long, requires = "repair")]
        take_over_listener: bool,
        /// Environment variable holding the bearer. With --item omitted this
        /// names the assignment auth-check reads from --env-file; with
        /// --repair it is the assignment synchronized from the item.
        #[arg(long)]
        variable: Option<String>,
        /// Runtime env file holding (or, with --repair, receiving) the
        /// bearer assignment named by --variable.
        #[arg(long)]
        env_file: Option<String>,
        #[arg(long)]
        json: bool,
    },

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

pub async fn dispatch(command: ServiceCommands) -> Result<(), CmdError> {
    match command {
        ServiceCommands::Directory(sub) => crate::cli::directory::dispatch(sub).await,
        ServiceCommands::Catalog { json } => catalog(json).await,
        ServiceCommands::List {
            unowned,
            undeclared,
            json,
        } => {
            if unowned && undeclared {
                return Err(CmdError::usage(
                    "--unowned and --undeclared are two different questions; ask one at a time",
                ));
            }
            if undeclared {
                list_undeclared(json).await
            } else if unowned {
                list_unowned(json).await
            } else {
                list(json).await
            }
        }
        ServiceCommands::Bootout {
            label,
            host,
            domain,
            json,
        } => bootout(&label, &host, domain.as_deref(), json).await,
        ServiceCommands::Reap {
            host,
            command,
            apply,
            json,
        } => reap(&host, &command, apply, json).await,
        ServiceCommands::WatchSpawn {
            host,
            command,
            seconds,
            interval_ms,
            json,
        } => watch_spawn(&host, &command, seconds, interval_ms, json).await,
        ServiceCommands::LabelPrint {
            label,
            host,
            domain,
            json,
        } => label_print(&label, &host, domain.as_deref(), json).await,
        ServiceCommands::Verify { host, local, json } => {
            if local {
                crate::cli::service_verify::verify_local(json).await
            } else {
                crate::cli::service_verify::verify(host.as_deref(), json).await
            }
        }
        ServiceCommands::Converge {
            target,
            binary,
            apply,
            json,
        } => crate::cli::service_converge::converge(&target, binary.as_deref(), apply, json).await,
        ServiceCommands::OnboardingCatalog => onboarding_catalog().await,
        ServiceCommands::Status { name, json } => status(&name, json).await,
        ServiceCommands::RefreshImage {
            name,
            if_needed,
            json,
        } => crate::cli::service_refresh_image::refresh_image(&name, if_needed, json).await,
        ServiceCommands::Update {
            name,
            host,
            from_artifact,
            from_archive,
            rollback_to,
            refresh_image,
            json,
        } => {
            update(
                &name,
                &host,
                from_artifact.as_deref(),
                from_archive.as_deref(),
                rollback_to.as_deref(),
                refresh_image,
                json,
            )
            .await
        }
        ServiceCommands::Release {
            name,
            host,
            product,
            version,
            readiness_url,
            readiness_timeout_seconds,
            reload_unit,
            require_release_version,
            supersede_unit,
            json,
        } => {
            release(ServiceReleaseOptions {
                name: &name,
                host: &host,
                product: &product,
                version: &version,
                readiness_url: readiness_url.as_deref(),
                readiness_timeout_seconds,
                reload_unit,
                require_release_version,
                supersede_unit: supersede_unit.as_deref(),
                supersede_same_label_user: false,
                json,
                emit: true,
            })
            .await
        }
        ServiceCommands::Show { name, host, json } => show(&name, host.as_deref(), json).await,
        ServiceCommands::RepairRunnerRuntime { name, host, json } => {
            let services = declared_matching(&name, Some(&host)).await?;
            let target = host_channel::canonical_target(&host).await.map_err(click)?;
            let runner = production_runner();
            for managed in &services {
                let report =
                    crate::deploy::host_precheck_runner::repair_runtime(&target, managed, &runner)
                        .await
                        .map_err(click)?;
                if json {
                    print_json(&report)?;
                } else {
                    print!("{}", report["stdout"].as_str().unwrap_or_default());
                }
            }
            Ok(())
        }
        ServiceCommands::Stop {
            name,
            host,
            listener_url,
            json,
        } => stop(&name, host.as_deref(), listener_url.as_deref(), json).await,
        ServiceCommands::Restart {
            name,
            host,
            take_over_listener,
            recovery_unit,
            json,
        } => {
            restart(
                &name,
                host.as_deref(),
                take_over_listener.as_deref(),
                recovery_unit.as_deref(),
                json,
            )
            .await
        }
        ServiceCommands::SecretSync {
            name,
            host,
            item,
            field,
            variable,
            env_file,
            restart,
            json,
        } => {
            secret_sync(SecretSyncOptions {
                name: &name,
                host: &host,
                item: &item,
                field: &field,
                variable: &variable,
                env_file: &env_file,
                restart_after_sync: restart,
                as_json: json,
            })
            .await
        }
        ServiceCommands::FileSync {
            name,
            host,
            source_file,
            target_file,
            executable,
            json,
        } => {
            file_sync(FileSyncOptions {
                name: &name,
                host: &host,
                source_file: &source_file,
                target_file: &target_file,
                executable,
                as_json: json,
            })
            .await
        }
        ServiceCommands::FileFetch {
            name,
            host,
            source_file,
            dest_file,
            json,
        } => {
            file_fetch(FileFetchOptions {
                name: &name,
                host: &host,
                source_file: &source_file,
                dest_file: dest_file.as_deref(),
                as_json: json,
            })
            .await
        }
        ServiceCommands::EnvSet {
            name,
            host,
            key,
            env_file,
            value_file,
            json,
        } => {
            env_set(EnvSetOptions {
                name: &name,
                host: &host,
                key: &key,
                env_file: &env_file,
                value_file: &value_file,
                as_json: json,
            })
            .await
        }
        ServiceCommands::EnvUnset {
            name,
            host,
            key,
            env_file,
            json,
        } => {
            env_unset(EnvUnsetOptions {
                name: &name,
                host: &host,
                key: &key,
                env_file: &env_file,
                as_json: json,
            })
            .await
        }
        ServiceCommands::EnvShow {
            name,
            host,
            env_file,
            reveal,
            json,
        } => {
            env_show(EnvShowOptions {
                name: &name,
                host: &host,
                env_file: &env_file,
                reveal: reveal.as_deref(),
                as_json: json,
            })
            .await
        }
        ServiceCommands::EndpointCheck {
            name,
            host,
            env_file,
            json,
        } => {
            endpoint_check(EndpointCheckOptions {
                name: &name,
                host: &host,
                env_file: &env_file,
                as_json: json,
            })
            .await
        }
        ServiceCommands::Serving {
            name,
            host,
            ports,
            json,
        } => {
            serving(ServingOptions {
                name: &name,
                host: &host,
                ports: &ports,
                as_json: json,
            })
            .await
        }
        ServiceCommands::GrantSync {
            name,
            host,
            consumer,
            capabilities,
            vault_file,
            token_file,
            ttl_seconds,
            audience,
            json,
        } => {
            grant_sync(GrantSyncOptions {
                name: &name,
                host: &host,
                consumer: &consumer,
                capabilities: &capabilities,
                token_file: &token_file,
                vault_file: &vault_file,
                ttl_seconds,
                audience: audience.as_deref(),
                as_json: json,
            })
            .await
        }
        ServiceCommands::TokenFileSync {
            name,
            host,
            item,
            field,
            token_file,
            json,
        } => {
            token_file_sync(TokenFileSyncOptions {
                name: &name,
                host: &host,
                item: &item,
                field: &field,
                token_file: &token_file,
                as_json: json,
            })
            .await
        }
        ServiceCommands::AuthCheck {
            name,
            host,
            item,
            field,
            consumer,
            token_file,
            url,
            repair,
            take_over_listener,
            post_empty_json,
            expect_status,
            variable,
            env_file,
            json,
        } => {
            auth_check(AuthCheckOptions {
                name: &name,
                host: &host,
                item: item.as_deref(),
                field: &field,
                consumer: consumer.as_deref(),
                token_file: token_file.as_deref(),
                url: &url,
                post_empty_json,
                expect_status,
                repair,
                take_over_listener,
                variable: variable.as_deref(),
                env_file: env_file.as_deref(),
                as_json: json,
            })
            .await
        }
        ServiceCommands::Adopt {
            unit,
            host,
            host_heuristic,
            json,
        } => adopt(&unit, host.as_deref(), host_heuristic.as_deref(), json).await,
        ServiceCommands::Onboarding {
            name,
            host,
            product_id,
            display_name,
            repository,
            surfaces,
            first_success_fact,
            onboarding_kind,
            status,
            json,
        } => {
            onboarding(OnboardingOptions {
                name: &name,
                host: &host,
                product_id: &product_id,
                display_name: &display_name,
                repository: &repository,
                surfaces,
                first_success_fact: &first_success_fact,
                onboarding_kind: &onboarding_kind,
                status: &status,
                as_json: json,
            })
            .await
        }
        ServiceCommands::Retire { unit, host, json } => retire(&unit, &host, json).await,
        ServiceCommands::HandoffReleaseControl {
            service,
            host,
            product,
            json,
        } => handoff_release_control(&service, &host, &product, json).await,
        ServiceCommands::Remove { unit, host, json } => remove(&unit, &host, json).await,
        ServiceCommands::Deploy {
            name,
            host,
            host_heuristic,
            from,
            from_artifact,
            args,
            launchd_label,
            as_launch_agent,
            json,
        } => {
            deploy(DeployOptions {
                name: &name,
                host: host.as_deref(),
                host_heuristic: host_heuristic.as_deref(),
                from,
                from_artifact,
                args: &args,
                launchd_label: launchd_label.as_deref(),
                as_launch_agent,
                as_json: json,
            })
            .await
        }
        ServiceCommands::Declare { file, json } => declare(&file, json).await,
        ServiceCommands::Ensure {
            name,
            host,
            from,
            args,
            env,
            reason,
            as_daemon,
            as_launch_agent,
            json,
        } => {
            ensure(EnsureOptions {
                name: &name,
                host: &host,
                from: from.as_deref(),
                args: &args,
                env: &env,
                reason: &reason,
                as_daemon,
                as_launch_agent,
                as_json: json,
            })
            .await
        }
        ServiceCommands::Logs {
            name,
            host,
            lines,
            json,
        } => logs(&name, host.as_deref(), lines, json).await,
        ServiceCommands::Env { name, host, json } => env(&name, host.as_deref(), json).await,
    }
}

// ---------------------------------------------------------------------------
// Shared resolution
// ---------------------------------------------------------------------------

fn click(exc: DeployError) -> CmdError {
    CmdError::click(exc.to_string())
}
async fn resolve_placement(
    host: Option<&str>,
    host_heuristic: Option<&str>,
) -> Result<(crate::targets::ComputeTarget, Option<String>), CmdError> {
    let resolved_host = if let Some(host) = host {
        host.to_string()
    } else if let Some(heuristic) = host_heuristic {
        let registry = targets::load_registry_auto()
            .await
            .map_err(|exc| CmdError::click(exc.to_string()))?;
        registry
            .lookup_host_heuristic(heuristic)
            .map(|target| target.name.clone())
            .ok_or_else(|| {
                CmdError::click(format!(
                    "host heuristic '{heuristic}' matches no local registry target"
                ))
            })?
    } else {
        return Err(CmdError::click(
            "either --host or --host-heuristic is required".to_string(),
        ));
    };
    let target = host_channel::canonical_target(&resolved_host)
        .await
        .map_err(click)?;
    Ok((target, host_heuristic.map(str::to_string)))
}

/// Reuse the host command's provider-neutral beacon store selection.
async fn beacon_store() -> Result<JobStorage, CmdError> {
    super::host::beacon_store().await
}

/// The declared managed set matching NAME, without touching beacons.
///
/// The write-side commands need the declaration — its unit id and its
/// unit-file path — not its state, so they must not pay for a beacon read
/// per host to get it.
pub(crate) async fn declared_matching(
    name: &str,
    host: Option<&str>,
) -> Result<Vec<ManagedService>, CmdError> {
    if let Some(host) = host {
        // Resolve the host first so an unknown or non-local target reports
        // the registry's own precise refusal rather than "no such service".
        host_channel::canonical_target(host).await.map_err(click)?;
    }
    let registry = registry::read_registry().await?;
    let mut found: Vec<ManagedService> = Vec::new();
    for target in registry.local_targets() {
        if host.is_some_and(|host| target.name != host) {
            continue;
        }
        found.extend(
            service::declared_services(target)
                .into_iter()
                .filter(|declared| declared.matches(name)),
        );
    }
    if found.is_empty() {
        return Err(unmanaged(name, host));
    }
    Ok(found)
}

fn unmanaged(name: &str, host: Option<&str>) -> CmdError {
    match host {
        Some(host) => CmdError::click(format!(
            "{name} is not a registry-managed service on {host}"
        )),
        None => CmdError::click(format!("no registry-managed service named {name}")),
    }
}

/// `-` for an empty cell, the spelling `monitor/host_health.rs` already
/// prints for a beacon field it does not have.
fn dash(value: &str) -> String {
    if value.is_empty() {
        "-".to_string()
    } else {
        value.to_string()
    }
}

fn print_json(value: &Value) -> Result<(), CmdError> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

// ---------------------------------------------------------------------------
// Read commands
// ---------------------------------------------------------------------------

async fn list(json: bool) -> Result<(), CmdError> {
    let store = beacon_store().await?;
    let rows = service::list_services(&store).await.map_err(click)?;
    render_status(&rows, json, &[])
}

/// `service list --unowned` — product processes no unit owns, fleet-wide.
///
/// The one read in this group that cannot come off the beacons: a beacon
/// reports the units the host was told about, and an unowned process is by
/// construction in nobody's declaration. So it is one read-only ssh per
/// kind=local host, and a host that will not answer is named on stderr rather
/// than dropped — "no unowned processes" and "nobody looked" are the fold this
/// whole group refuses to make.
/// One host's per-candidate ownership verdicts: the host, then
/// `(pid, "owned"|"unowned", the ancestor pid launchd claimed)` for each.
type HostVerdicts = (String, Vec<(String, String, String)>);

async fn list_unowned(json: bool) -> Result<(), CmdError> {
    let registry = registry::read_registry().await?;
    let runner = production_runner();
    let mut found: Vec<service::UnownedProcess> = Vec::new();
    let mut accounts: Vec<String> = Vec::new();
    let mut verdicts: Vec<HostVerdicts> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    for target in registry.local_targets() {
        match service::unowned_processes(target, &runner).await {
            Ok(scan) => {
                accounts.push(scan.account(&target.name));
                verdicts.push((target.name.clone(), scan.judged.clone()));
                found.extend(scan.processes);
            }
            Err(exc) => failures.push(format!("{}: {exc}", target.name)),
        }
    }
    if json {
        let payload: Vec<Value> = found.iter().map(service::UnownedProcess::to_json).collect();
        let judged: Vec<Value> = verdicts
            .iter()
            .flat_map(|(host, rows)| {
                rows.iter().map(move |(pid, verdict, owner)| {
                    json!({"host": host, "pid": pid, "verdict": verdict, "claimed_by": owner})
                })
            })
            .collect();
        print_json(&json!({"unowned": payload, "searched": accounts, "judged": judged}))?;
    } else {
        let cells: Vec<Vec<String>> = found
            .iter()
            .map(|process| {
                vec![
                    process.host.clone(),
                    process.pid.clone(),
                    process.product_guess(),
                    dash(&process.started_at),
                    process.command.clone(),
                ]
            })
            .collect();
        table::print(
            &["HOST", "PID", "PRODUCT_GUESS", "STARTED_AT", "COMMAND"],
            &cells,
        );
        // Printed on every run, not only the empty one: a table with three rows
        // and a root that matched nothing is the same unread answer as an empty
        // table, one root later.
        for account in &accounts {
            println!("searched {account}");
        }
        for (host, judged) in &verdicts {
            for (pid, verdict, owner) in judged {
                println!("judged {host}: pid {pid} {verdict} (launchd claimed {owner})");
            }
        }
    }
    fail_if_any(&failures, "scan for unowned processes")
}

/// `service list --undeclared` — launchd jobs a host has loaded that the
/// registry does not declare, fleet-wide.
///
/// The third question, and the one that had no answer. [`list`] walks the
/// document and asks the host about each entry. [`list_unowned`] walks the
/// processes and asks launchd who owns them. A unit launchd has LOADED and the
/// document has never heard of is in neither set, so nothing in this binary
/// could name one.
///
/// charless-mac-mini was running three queue agents at once in that blind spot:
/// `com.wisent.compute.service.stado-agent-mini`, the only one the registry
/// declares, plus `com.wisent.compute.agent.charless-mac-mini` from
/// `stado bootstrap --local`'s label convention and
/// `com.wisent.compute.service.stado-queue-agent` from a third. All three
/// published capacity for the same consumer id, so whichever wrote last decided
/// what the host answered — and the oldest of them, three days into a stale
/// binary, refused 55 pinned jobs for a week while every report in this group
/// said the declared agent was fine.
///
/// An empty answer means the hosts were asked and had nothing, because a host
/// that will not answer is named on stderr and makes the command fail.
///
/// It also means the whole host was asked. Until 2026-09-01 this command
/// enumerated only labels under `com.wisent.`, so its empty answer was a fact
/// about that prefix and was read as a fact about the machine:
/// `com.stado.agent.charless-mac-mini` was loaded on the always-on mac, was the
/// only label on it outside the prefix, held the pid rewriting the janitor's
/// state file every interval — and this command said the host had nothing
/// undeclared. Every row is now enumerated and classified; the prefix chooses
/// the sentence, never the population.
async fn list_undeclared(json: bool) -> Result<(), CmdError> {
    let registry = registry::read_registry().await?;
    let runner = production_runner();
    let mut found: Vec<service::UndeclaredUnit> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    for target in registry.local_targets() {
        match service::undeclared_units(target, &runner).await {
            Ok(units) => found.extend(units),
            Err(exc) => failures.push(format!("{}: {exc}", target.name)),
        }
    }
    // A label the fleet never named but the host ties to the fleet anyway is
    // the more interesting class and used to be the invisible one, so it reads
    // first. This is the order rows are printed in and nothing else.
    found.sort_by(|left, right| {
        let rank = |unit: &service::UndeclaredUnit| match unit.classification() {
            "outside-fleet-prefix" => 0,
            "undeclared" => 1,
            _ => 2,
        };
        rank(left)
            .cmp(&rank(right))
            .then_with(|| left.host.cmp(&right.host))
            .then_with(|| left.label.cmp(&right.label))
    });
    if json {
        // Every row, every class. The JSON answer is the complete one, so
        // nothing below can be the only place a label exists.
        let payload: Vec<Value> = found.iter().map(service::UndeclaredUnit::to_json).collect();
        print_json(&json!({"undeclared": payload}))?;
    } else {
        // The table prints the jobs this fleet put on the host and cannot
        // account for. `unaffiliated` rows are counted below instead: on
        // charless-mac-mini they are 494 of 537 loaded labels, all of them the
        // platform's own, and printing them beside six real findings is the
        // same disservice the prefix filter did by another route. They are read,
        // classified and counted, and `--json` carries every one of them.
        let actionable: Vec<&service::UndeclaredUnit> =
            found.iter().filter(|unit| !unit.accounted_for()).collect();
        let cells: Vec<Vec<String>> = actionable
            .iter()
            .map(|unit| {
                vec![
                    unit.host.clone(),
                    unit.classification().to_string(),
                    unit.label.clone(),
                    dash(&unit.pid),
                    unit.status.clone(),
                    // What the process IS running, and only then what its file
                    // declares. Reading only the declaration is how a job could
                    // be seen and not identified: the pid rewriting the
                    // janitor's state file on charless-mac-mini is named
                    // `com.stado.agent.charless-mac-mini`, and only its argv
                    // says it is `python3.12 -m stado.cli agent`, a program no
                    // release of this binary can ever change.
                    dash(if unit.running_program.is_empty() {
                        &unit.program
                    } else {
                        &unit.running_program
                    }),
                    dash(&unit.path),
                ]
            })
            .collect();
        table::print(
            &[
                "HOST",
                "CLASS",
                "LABEL",
                "PID",
                "LAST_EXIT",
                "RUNS",
                "UNIT_FILE",
            ],
            &cells,
        );
        // The census, so an empty table and a table whose interesting rows are
        // outnumbered both read honestly — and so that the widening is
        // auditable: these numbers are the proof the host was asked about every
        // label rather than about one prefix.
        let count = |wanted: &str| {
            found
                .iter()
                .filter(|unit| unit.classification() == wanted)
                .count()
        };
        println!(
            "{} loaded label(s) the registry does not declare: {} outside the fleet prefix but \
             tied to it by unit file or program, {} under the prefix, {} unaffiliated with this \
             fleet and not listed above (`--json` carries every row)",
            found.len(),
            count("outside-fleet-prefix"),
            count("undeclared"),
            count("unaffiliated"),
        );
    }
    fail_if_any(&failures, "scan for undeclared units")
}

/// `service bootout LABEL --host HOST [--domain system|user]` — take one exact
/// unit out of launchd or systemd, whether the registry declares it or not.
///
/// Without `--domain`, the system scope is tried first and the calling
/// account's scope only when the system manager holds no exact unit by that
/// name. Explicit `user` is what ends a stale user unit while leaving its
/// canonical system sibling running.
async fn bootout(
    label: &str,
    host: &str,
    domain: Option<&str>,
    json: bool,
) -> Result<(), CmdError> {
    let scope = service::BootoutScope::parse(domain).map_err(click)?;
    let target = host_channel::canonical_target(host).await.map_err(click)?;
    let runner = production_runner();
    let (state, detail) = service::bootout_label(&target, label, scope, &runner)
        .await
        .map_err(click)?;
    if json {
        return print_json(&json!({
            "host": target.name,
            "label": label,
            "state": state,
            "detail": detail,
        }));
    }
    table::print(
        &["HOST", "LABEL", "STATE", "DETAIL"],
        &[vec![
            target.name.clone(),
            label.to_string(),
            state.clone(),
            detail.clone(),
        ]],
    );
    if state == "refused" || state == "failed" {
        return Err(CmdError::click(format!(
            "{}: {label} {state}: {detail}",
            target.name
        )));
    }
    Ok(())
}

/// `service reap --host HOST [--apply]` — end the product processes on HOST
/// that no declared unit owns.
///
/// Ownership is the registry's, not launchd's. `list --unowned` asks whether ANY
/// launchd job claims a process, and on a mac that set is about a thousand pids,
/// so a duplicate running under a label the document never declared reads as
/// owned and is left alone. This asks the question that matters: is this process
/// the one the document says should be running.
async fn reap(host: &str, command: &str, apply: bool, json: bool) -> Result<(), CmdError> {
    let target = host_channel::canonical_target(host).await.map_err(click)?;
    let runner = production_runner();
    let (reaped, kept) = service::reap_undeclared_processes(&target, command, apply, &runner)
        .await
        .map_err(click)?;
    if json {
        let payload: Vec<Value> = reaped.iter().map(service::ReapedProcess::to_json).collect();
        return print_json(&json!({
            "host": target.name,
            "applied": apply,
            "kept_pids": kept,
            "reaped": payload,
        }));
    }
    let cells: Vec<Vec<String>> = reaped
        .iter()
        .map(|process| {
            vec![
                process.pid.clone(),
                process.outcome.clone(),
                dash(&process.started_at),
                process.command.clone(),
            ]
        })
        .collect();
    table::print(&["PID", "OUTCOME", "STARTED_AT", "COMMAND"], &cells);
    // The kept set is the other half of the verdict: an empty table with no
    // kept pid means the declared units are not running either, which is a
    // different problem from a clean host.
    println!(
        "{}: declared units hold pid(s) [{}]{}",
        target.name,
        if kept.is_empty() { "none" } else { &kept },
        if apply {
            ""
        } else {
            "; nothing was signalled (pass --apply)"
        }
    );
    let stubborn = reaped
        .iter()
        .filter(|process| process.outcome == "still_running")
        .count();
    if stubborn > 0 {
        return Err(CmdError::click(format!(
            "{}: {stubborn} process(es) did not end on SIGTERM; their rows name each pid",
            target.name
        )));
    }
    Ok(())
}

/// `service watch-spawn --host HOST --command SUBSTRING` — name the parent of
/// the next matching process, while that parent still exists.
///
/// The report leads with the parent because the parent is the whole question.
/// A row reading `ppid 1` with `launchd` as the parent is not a failure of
/// this command: it means the arrival was already reparented when the sample
/// caught it, and the answer is to sample faster.
async fn watch_spawn(
    host: &str,
    command: &str,
    seconds: u64,
    interval_ms: u64,
    json: bool,
) -> Result<(), CmdError> {
    let target = host_channel::canonical_target(host).await.map_err(click)?;
    let runner = production_runner();
    let report = service_spawn_watch::watch_spawns(&target, command, seconds, interval_ms, &runner)
        .await
        .map_err(click)?;
    if json {
        return print_json(&json!({
            "host": report.host,
            "matched": report.matched,
            "seconds": report.seconds,
            "interval_ms": report.interval_ms,
            "samples": report.samples,
            "elapsed_seconds": report.elapsed_seconds,
            "unsupported": report.unsupported,
            "baseline": report.baseline.iter().map(|entry| entry.row.to_json()).collect::<Vec<_>>(),
            "arrivals": report.arrivals.iter().map(service_spawn_watch::Arrival::to_json).collect::<Vec<_>>(),
        }));
    }
    if let Some(system) = &report.unsupported {
        println!(
            "{}: spawn watch is Darwin-only; the host reports {system}",
            report.host
        );
        return Ok(());
    }
    let baseline: Vec<Vec<String>> = report
        .baseline
        .iter()
        .map(|entry| {
            vec![
                entry.row.pid.clone(),
                entry.row.ppid.clone(),
                entry.row.started_at.clone(),
                entry.row.command.clone(),
            ]
        })
        .collect();
    println!("already running when the watch opened:");
    table::print(&["PID", "PPID", "STARTED_AT", "COMMAND"], &baseline);
    if report.arrivals.is_empty() {
        // A watch that saw nothing is a result, not a dud: it says the thing
        // that was respawning has stopped, or never fired in this window.
        println!(
            "\n{}: no process matching {:?} started in {}s across {} samples",
            report.host, report.matched, report.elapsed_seconds, report.samples
        );
        return Ok(());
    }
    for arrival in &report.arrivals {
        println!(
            "\narrival {} at +{}s: pid {} ppid {} — {}",
            arrival.sequence,
            arrival.after_seconds,
            arrival.row.pid,
            arrival.row.ppid,
            arrival.row.command
        );
        let cells: Vec<Vec<String>> = arrival
            .ancestry
            .iter()
            .map(|ancestor| {
                vec![
                    ancestor.depth.to_string(),
                    ancestor.row.pid.clone(),
                    ancestor.row.ppid.clone(),
                    if ancestor.alive { "yes" } else { "no" }.to_string(),
                    ancestor.row.started_at.clone(),
                    ancestor.row.command.clone(),
                ]
            })
            .collect();
        table::print(
            &["DEPTH", "PID", "PPID", "ALIVE", "STARTED_AT", "COMMAND"],
            &cells,
        );
        match arrival.parent() {
            Some(parent) => println!(
                "  parent: pid {} ({}) — {}",
                parent.row.pid,
                if parent.alive {
                    "still running"
                } else {
                    "already exited"
                },
                parent.row.command
            ),
            None => println!("  parent: not in the snapshot that caught it; sample faster"),
        }
    }
    println!(
        "\n{}: {} arrival(s) in {}s across {} samples",
        report.host,
        report.arrivals.len(),
        report.elapsed_seconds,
        report.samples
    );
    Ok(())
}

/// `service label-print LABEL --host HOST` — what the host init system holds
/// under one exact unit identity, asked rather than enumerated.
///
/// Exits non-zero when neither launchd nor systemd holds the named unit.
async fn label_print(
    label: &str,
    host: &str,
    domain: Option<&str>,
    json: bool,
) -> Result<(), CmdError> {
    let target = host_channel::canonical_target(host).await.map_err(click)?;
    let scope = service::BootoutScope::parse(domain).map_err(click)?;
    let runner = production_runner();
    let state = service_label_print::inspect_label(&target, label, scope, &runner)
        .await
        .map_err(click)?;
    if json {
        print_json(&state.to_json())?;
    }
    if let Some(system) = &state.unsupported {
        if !json {
            println!("{}: label-print does not support {system}", state.host);
        }
        return Ok(());
    }
    if !state.loaded() {
        if !state.read_failures.is_empty() {
            let detail = state
                .read_failures
                .iter()
                .map(|failure| {
                    format!(
                        "{} exited {}: {}",
                        failure.domain, failure.exit_code, failure.detail
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            return Err(CmdError::click(format!(
                "{}: could not determine whether {label} is loaded: {detail}",
                state.host
            )));
        }
        if !json {
            println!(
                "{}: the init system holds no unit under {label} in the {} domain(s)",
                state.host,
                domain.unwrap_or("system and user")
            );
        }
        return Err(CmdError::click(format!(
            "{}: {label} is not loaded",
            state.host
        )));
    }
    if json {
        return Ok(());
    }
    let mut rows = vec![
        vec![
            "domain".to_string(),
            dash(state.domain.as_deref().unwrap_or("")),
        ],
        vec!["pid".to_string(), dash(state.pid.as_deref().unwrap_or(""))],
        vec![
            "state".to_string(),
            dash(state.state.as_deref().unwrap_or("")),
        ],
        vec![
            "last exit code".to_string(),
            dash(state.last_exit_code.as_deref().unwrap_or("")),
        ],
        vec![
            "runs".to_string(),
            dash(state.runs.as_deref().unwrap_or("")),
        ],
        vec![
            "path".to_string(),
            dash(state.path.as_deref().unwrap_or("")),
        ],
        vec!["program".to_string(), dash(state.runs().unwrap_or(""))],
    ];
    for failure in &state.read_failures {
        rows.push(vec![
            format!("{} read failure", failure.domain),
            format!("exit {}: {}", failure.exit_code, failure.detail),
        ]);
    }
    if state.event_read_status.is_some() {
        rows.extend([
            vec![
                "stdout path".to_string(),
                dash(state.stdout_path.as_deref().unwrap_or("")),
            ],
            vec![
                "stderr path".to_string(),
                dash(state.stderr_path.as_deref().unwrap_or("")),
            ],
            vec![
                "recent launchd events".to_string(),
                dash(
                    &state
                        .recent_events
                        .iter()
                        .map(String::as_str)
                        .collect::<Vec<_>>()
                        .join(" | "),
                ),
            ],
            vec![
                "launchd event read".to_string(),
                dash(state.event_read_status.as_deref().unwrap_or("")),
            ],
        ]);
    }
    rows.extend([
        vec![
            "unit file state".to_string(),
            dash(state.unit_file_state.as_deref().unwrap_or("")),
        ],
        vec![
            "restart".to_string(),
            dash(state.restart.as_deref().unwrap_or("")),
        ],
        vec![
            "triggers".to_string(),
            dash(state.triggers.as_deref().unwrap_or("")),
        ],
        vec![
            "triggered by".to_string(),
            dash(state.triggered_by.as_deref().unwrap_or("")),
        ],
        vec![
            "part of".to_string(),
            dash(state.part_of.as_deref().unwrap_or("")),
        ],
    ]);
    table::print(&["FIELD", "VALUE"], &rows);
    // A loaded unit whose file is gone is the shape no directory scan can
    // report, so it is called out rather than left to be inferred from a path.
    if state.path.is_none() {
        println!(
            "{}: {label} is loaded with no unit file recorded — nothing that scans directories can see it",
            state.host
        );
    }
    Ok(())
}

async fn onboarding_catalog() -> Result<(), CmdError> {
    let store = beacon_store().await?;
    let rows = service::list_services(&store).await.map_err(click)?;
    let services: Vec<Value> = rows
        .iter()
        .filter(|row| row.service.source == SOURCE_REGISTRY && row.service.onboarding.is_some())
        .map(ServiceStatus::to_json)
        .collect();
    print_json(&json!({"schema_version": 1, "services": services}))
}

async fn status(name: &str, json: bool) -> Result<(), CmdError> {
    let store = beacon_store().await?;
    let rows = service::find_services(&store, name).await.map_err(click)?;
    if rows.is_empty() {
        return Err(unmanaged(name, None));
    }
    // A `failed` row is the beacon saying "it died" — more often than not
    // with an empty detail. The why lives on the host: launchd's last exit
    // status, and the stderr the unit wrote before it went. Gather it
    // best-effort over the read-only channels; `status` must still answer
    // when the host cannot.
    let runner = production_runner();
    let mut failures: Vec<FailureEvidence> = Vec::new();
    for row in &rows {
        if row.state == service::STATE_FAILED {
            failures.push(failure_evidence(row, &runner).await);
        }
    }
    render_status(&rows, json, &failures)
}

/// How many stderr lines one `failure:` block may carry.
const FAILURE_STDERR_LINES: usize = 10;

/// Why one `failed` unit died, gathered best-effort from the host itself.
/// Every read can fail — the host may be the thing that is broken — so a
/// failed read degrades to a note, never to a failed `status`.
struct FailureEvidence {
    host: String,
    unit: String,
    /// launchd's last exit status for the label, when `launchctl list`
    /// carried it.
    last_exit: Option<String>,
    /// Where the stderr tail came from, or the reason there is none.
    error_origin: Option<String>,
    error_lines: Vec<String>,
    /// Why gathering failed, when it did.
    note: Option<String>,
}

impl FailureEvidence {
    fn push_note(&mut self, note: String) {
        match &mut self.note {
            Some(existing) => {
                existing.push_str("; ");
                existing.push_str(&note);
            }
            None => self.note = Some(note),
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "last_exit": self.last_exit,
            "error_origin": self.error_origin,
            "error_lines": self.error_lines,
            "note": self.note,
        })
    }
}

/// The `Status` column of one label in `launchctl list` output: launchd's
/// last exit status for the job while nothing runs under it. Columns are
/// PID, Status, Label, tab-separated; the header row never collides with a
/// real label.
fn launchctl_last_exit(stdout: &str, label: &str) -> Option<String> {
    stdout.lines().find_map(|line| {
        let mut columns = line.split('\t');
        let _pid = columns.next()?;
        let status = columns.next()?;
        let name = columns.next()?;
        (name == label).then(|| status.to_string())
    })
}

async fn failure_evidence(row: &ServiceStatus, runner: &crate::deploy::Runner) -> FailureEvidence {
    let unit = row.service.unit_id().to_string();
    let mut evidence = FailureEvidence {
        host: row.service.host.clone(),
        unit: unit.clone(),
        last_exit: None,
        error_origin: None,
        error_lines: Vec::new(),
        note: None,
    };
    // The last exit status rides the approved read-only allowlist: the
    // exact `launchctl list` entry, never a shell.
    let words = vec!["launchctl".to_string(), "list".to_string()];
    match host_exec::exec_host(&row.service.host, &words, runner).await {
        Ok(report)
            if report.get("status").and_then(Value::as_str) == Some(host_exec::OK_STATUS) =>
        {
            let stdout = report
                .get("stdout")
                .and_then(Value::as_str)
                .unwrap_or_default();
            evidence.last_exit = launchctl_last_exit(stdout, &unit);
        }
        Ok(report) => {
            let status = report
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            evidence.push_note(format!("last exit unreadable: {status}"));
        }
        Err(exc) => evidence.push_note(format!("last exit unreadable: {exc}")),
    }
    // The stderr tail comes from the same logs path `service logs` uses,
    // narrowed to the lines a failure block can show.
    match host_channel::canonical_target(&row.service.host).await {
        Ok(target) => {
            match service::tail_logs(&target, &row.service, 2 * FAILURE_STDERR_LINES, runner).await
            {
                Ok(log) => {
                    evidence.error_origin = log.error_origin;
                    evidence.error_lines = log
                        .error_body
                        .lines()
                        .take(FAILURE_STDERR_LINES)
                        .map(str::to_string)
                        .collect();
                }
                Err(exc) => evidence.push_note(format!("stderr unreadable: {exc}")),
            }
        }
        Err(exc) => evidence.push_note(format!("stderr unreadable: {exc}")),
    }
    evidence
}

fn render_status(
    rows: &[ServiceStatus],
    json: bool,
    failures: &[FailureEvidence],
) -> Result<(), CmdError> {
    // Read once for the whole table. The record is a file on this machine and
    // the answer is the same for every row in one rendering.
    let seen = observations::load();
    if json {
        let payload: Vec<Value> = rows
            .iter()
            .map(|row| {
                let mut entry = row.to_json();
                // Carried in the machine-readable form too, because the
                // consumers of `--json` are the dashboards and gates that
                // acted on a twelve-day-old `active` without ever being able
                // to see how old it was.
                let fact = observations::service_fact(&row.service.name, &row.service.host);
                entry["observed"] = json!(observations::describe_in(&seen, &fact));
                if let Some(failure) = failures.iter().find(|failure| {
                    failure.host == row.service.host && failure.unit == row.service.unit_id()
                }) {
                    entry["failure"] = failure.to_json();
                }
                entry
            })
            .collect();
        return print_json(&Value::Array(payload));
    }
    // The domain column appears only when at least one row is a system
    // LaunchDaemon: that is the one domain the approved channel cannot
    // bootstrap, and a fleet of user-domain units should not pay for a
    // column that says nothing.
    let show_domain = rows
        .iter()
        .any(|row| UnitDomain::from_path(&row.service.path).requires_privileged_bootstrap());
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            let fact = observations::service_fact(&row.service.name, &row.service.host);
            let mut cells = vec![
                row.service.host.clone(),
                row.service.name.clone(),
                row.service.unit_id().to_string(),
                row.service.source.clone(),
                row.state.clone(),
                dash(&row.reported_at),
                observations::describe_in(&seen, &fact),
                dash(&row.detail),
            ];
            if show_domain {
                cells.insert(3, dash(UnitDomain::from_path(&row.service.path).as_str()));
            }
            cells
        })
        .collect();
    let mut headers = vec![
        "HOST",
        "SERVICE",
        "UNIT",
        "SOURCE",
        "STATE",
        "REPORTED_AT",
        "OBSERVED",
        "DETAIL",
    ];
    if show_domain {
        headers.insert(3, "DOMAIN");
    }
    table::print(&headers, &cells);
    // A unit declared in a domain its host cannot have, named where the
    // operator is already reading the table it is missing from. This is a
    // fact about the document, so it prints for every row that carries it
    // whatever the beacon said — including the `missing` rows, where the
    // declaration is the reason the beacon reports nothing.
    for row in rows {
        if let Some(misdeclared) = &row.misdeclared_domain {
            println!("declaration: {}", misdeclared.sentence());
        }
    }
    for failure in failures {
        let exit = match &failure.last_exit {
            Some(exit) => format!("last launchd exit {exit}"),
            None => "last launchd exit unknown".to_string(),
        };
        println!("failure: {} {}: {}", failure.host, failure.unit, exit);
        // A failed system LaunchDaemon has exactly one repair over the
        // approved channel, and it has conditions; say which, here, where the
        // operator is reading why.
        if rows.iter().any(|row| {
            row.service.host == failure.host
                && row.service.unit_id() == failure.unit.as_str()
                && UnitDomain::from_path(&row.service.path).requires_privileged_bootstrap()
        }) {
            println!(
                "  unit: system LaunchDaemon — `service restart` can only end its process and let \
                 launchd's KeepAlive replace it; loading it takes sudo on the host"
            );
        }
        if let Some(error_origin) = &failure.error_origin {
            println!("  stderr: {error_origin}");
        }
        for line in &failure.error_lines {
            println!("  {line}");
        }
        if let Some(note) = &failure.note {
            println!("  note: {note}");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Restart
// ---------------------------------------------------------------------------
pub(crate) async fn host_sudo_password(
    target: &crate::targets::ComputeTarget,
) -> Result<Option<String>, CmdError> {
    let Some(item) = target.account_ref.as_deref() else {
        return Ok(None);
    };
    match crate::credential_store::read_string(item, "password").await {
        Ok(password) => Ok(password.filter(|value| !value.is_empty())),
        Err(broker_error) => owner_host_password(item).await.map_err(|owner_error| {
            CmdError::click(format!(
                "cannot read {item}#password for privileged lifecycle on {}: broker: \
                 {broker_error}; owner vault: {owner_error}",
                target.name
            ))
        }),
    }
}

/// Read a host account through the owner-controlled local vault when the
/// broker grant is stale. The secret stays in captured process memory and is
/// handed directly to SSH stdin; stdout is never forwarded.
async fn owner_host_password(item: &str) -> Result<Option<String>, String> {
    let home = std::env::var("HOME").map_err(|error| error.to_string())?;
    let skarbiec = std::path::Path::new(&home).join(".stado/bin/skarbiec");
    let vault = std::path::Path::new(&home).join(".stado/skarbiec.vault.json");
    let path = format!(
        "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:{}",
        std::env::var("PATH").unwrap_or_default()
    );
    let output = tokio::process::Command::new(&skarbiec)
        .args(["get", item, "--field", "password"])
        .env("SKARBIEC_VAULT_FILE", &vault)
        .env("PATH", path)
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .map_err(|error| format!("cannot run {}: {error}", skarbiec.display()))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let raw = String::from_utf8(output.stdout)
        .map_err(|error| format!("Skarbiec returned non-UTF-8 password bytes: {error}"))?;
    let password = match serde_json::from_str::<Value>(&raw) {
        Ok(document) => document
            .get("fields")
            .and_then(|fields| fields.get("password"))
            .and_then(Value::as_str)
            .map(str::to_string),
        Err(_) => Some(raw.trim_end_matches(['\n', '\r']).to_string()),
    };
    Ok(password.filter(|value| !value.is_empty()))
}

pub(crate) async fn restart(
    name: &str,
    host: Option<&str>,
    take_over_listener: Option<&str>,
    recovery_unit: Option<&str>,
    json: bool,
) -> Result<(), CmdError> {
    let services = declared_matching(name, host).await?;
    let runner = production_runner();
    let mut payload: Vec<Value> = Vec::new();
    let mut cells: Vec<Vec<String>> = Vec::new();
    let mut failures: Vec<String> = Vec::new();

    for declared in &services {
        let target = host_channel::canonical_target(&declared.host)
            .await
            .map_err(click)?;
        let sudo_password = if UnitDomain::from_path(&declared.path).requires_privileged_bootstrap()
        {
            host_sudo_password(&target).await?
        } else {
            None
        };
        if let Some(unit) = recovery_unit {
            service::stop_recovery_unit(&target, unit, &runner)
                .await
                .map_err(click)?;
        }
        if let Some(url) = take_over_listener {
            service::stop_service_with_password(
                &target,
                declared,
                sudo_password.as_deref(),
                &runner,
            )
            .await
            .map_err(click)?;
            let listener = service::reset_service_listener(&target, declared, url, &runner)
                .await
                .map_err(click)?;
            if !listener.succeeded("listener_stopped") && !listener.succeeded("listener_absent") {
                failures.push(format!("{}: {}", declared.host, listener.failure()));
                continue;
            }
        }
        let report = service::restart_service_with_password(
            &target,
            declared,
            sudo_password.as_deref(),
            &runner,
        )
        .await
        .map_err(click)?;
        if !report.succeeded("restarted") {
            failures.push(format!("{}: {}", declared.host, report.failure()));
        }
        cells.push(vec![
            declared.host.clone(),
            declared.unit_id().to_string(),
            // The domain, in the human output as well as in `--json`: a
            // restart that acted in `user/501` and a restart that acted in
            // `gui/501` are different operations, and the table used to print
            // neither.
            dash(&report.domain),
            dash(&report.status),
            dash(&report.detail),
        ]);
        let mut entry = report.to_json();
        entry["host"] = Value::from(declared.host.clone());
        payload.push(entry);
    }

    if json {
        print_json(&Value::Array(payload))?;
    } else {
        table::print(&["HOST", "UNIT", "DOMAIN", "STATUS", "DETAIL"], &cells);
    }
    fail_if_any(&failures, "restart")
}

async fn update(
    name: &str,
    host: &str,
    reference: Option<&str>,
    archive: Option<&str>,
    rollback_to: Option<&str>,
    refresh_image: bool,
    json: bool,
) -> Result<(), CmdError> {
    let target = host_channel::canonical_target(host).await.map_err(click)?;
    // The service must already be managed here: this moves a unit forward, and
    // silently installing a version for a unit nobody runs would look like a
    // deployment while changing nothing.
    let services = declared_matching(name, Some(host)).await?;
    if services.is_empty() {
        return Err(CmdError::click(format!(
            "{host} does not manage {name}; deploy it first"
        )));
    }
    // The registry name is the unit label; the artifact directory is whatever
    // the unit's own program path reads from. Deriving it from the unit means
    // the new version lands where the running one is actually read, instead of
    // beside it under a directory that only matches the name.
    let runner = production_runner();
    let declared = &services[usize::default()];
    // Archive membership follows the actual executable vector in the unit.
    // `service show` is deliberately human presentation and may contain
    // spaces in paths and arguments plus a resolved-link annotation.
    let unit = service::fetch_unit_file(&target, declared, &runner)
        .await
        .map_err(click)?;
    let observed = service::parse_unit_program(&unit)
        .map_err(click)?
        .ok_or_else(|| {
            CmdError::click(format!(
                "{} has no executable program in {}",
                declared.unit_id(),
                unit.path
            ))
        })?;
    let program = observed.as_str();
    let directory = program
        .split("/services/")
        .nth(usize::from(true))
        .and_then(|rest| rest.split('/').next())
        .filter(|segment| !segment.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| declared.name.clone());
    // Two sources, one install. A published artifact is the durable route; a
    // local archive is how a bundle reaches a host before there is an object
    // store the whole fleet can read, and it is checksummed on the far side the
    // same way rather than trusted for arriving.
    if let Some(version) = rollback_to {
        let script = format!(
            "set -euo pipefail\nname={}\nversion={}\n{ROLLBACK_BODY}",
            crate::deploy::shlex_quote(&directory),
            crate::deploy::shlex_quote(version),
        );
        let output = host_channel::run_script(&target, &script, &runner)
            .await
            .map_err(click)?;
        if !output.ok() {
            return Err(CmdError::click(format!(
                "{host}: {}",
                host_channel::last_error_line(&output, "rollback failed")
            )));
        }
        println!("{host}: {name} -> {version} (takes effect on the next restart)");
        return Ok(());
    }
    // The relink is the dangerous half: `current` moves and launchd's next
    // spawn reads a path that may not exist in the tree that just arrived.
    // Checking the archive's member list against the unit's own program path
    // costs one local read and is the difference between a refusal and an
    // outage. On 2026-09-04 the object API unit, whose program is
    // `current/darwin-arm/stado`, was pointed at a published stado archive
    // that holds exactly `bin/stado`; `current` relinked, launchd could not
    // spawn, the job left the system domain, and every `/api/object` read on
    // the fleet failed for eleven minutes.
    if let Some(path) = archive {
        let members = archive_members(path)?;
        refuse_archive_without_program(program, &members).map_err(CmdError::click)?;
    }
    let (installed, already_active) = match (reference, archive) {
        (Some(reference), None) => (
            install_from_artifact(&target, &directory, reference).await?,
            false,
        ),
        (None, Some(path)) => {
            let marker = format!("/services/{directory}/");
            let required = if let Some((_, rest)) = program.split_once(&marker) {
                rest.split_once('/')
                    .map(|(_, tail)| tail.to_string())
                    .ok_or_else(|| {
                        CmdError::click("managed service program has no archive member")
                    })?
            } else {
                let executable = std::path::Path::new(program)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| CmdError::click("managed service program has no filename"))?;
                format!("darwin-arm/{executable}")
            };
            if !std::path::Path::new(&required)
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_)))
            {
                return Err(CmdError::click(
                    "managed service archive member is not relative",
                ));
            }
            install_from_archive(&target, &directory, path, &required, &runner).await?
        }
        (None, None) => {
            return Err(CmdError::click(
                "update needs --from-artifact REF or --from-archive PATH",
            ))
        }
        (Some(_), Some(_)) => {
            return Err(CmdError::click(
                "--from-artifact and --from-archive are exclusive",
            ))
        }
    };
    let followed = if already_active {
        false
    } else {
        // A unit pinned to a version directory never sees an install: `current`
        // moves and the job keeps executing the path it was rendered with.
        follow_current(&target, declared, &directory, &runner).await?
    };
    let image_refresh = if refresh_image {
        let units = service::loaded_units(&target, &runner)
            .await
            .map_err(click)?;
        let before = units
            .iter()
            .find(|unit| unit.label == declared.unit_id())
            .ok_or_else(|| {
                CmdError::click(format!(
                    "{host}: {} is absent from the domain-bound launchd inventory",
                    declared.unit_id()
                ))
            })?;
        if before.pid.is_empty() {
            if before.loaded_domains.len() != 1 {
                return Err(CmdError::click(format!(
                    "{host}: {} has no live pid and {} loaded domains; refusing to guess a lifecycle action",
                    declared.unit_id(),
                    before.loaded_domains.len()
                )));
            }
            let started = service::restart_service(&target, declared, &runner)
                .await
                .map_err(click)?;
            if !started.succeeded("restarted") {
                return Err(CmdError::click(format!(
                    "{host}: {} was confirmed loaded without a live pid and did not start: {}",
                    declared.unit_id(),
                    started.failure()
                )));
            }
        }
        let script = format!(
            "set -euo pipefail\n\"$HOME/.stado/bin/stado\" service refresh-image {} \
             --if-needed --json",
            crate::deploy::shlex_quote(declared.unit_id()),
        );
        let output = host_channel::run_script(&target, &script, &runner)
            .await
            .map_err(click)?;
        if !output.ok() {
            return Err(CmdError::click(format!(
                "{host}: {}",
                host_channel::last_error_line(&output, "the running image did not converge")
            )));
        }
        serde_json::from_str::<Value>(output.stdout.trim()).map_err(|error| {
            CmdError::click(format!(
                "{host}: image refresh returned invalid JSON: {error}; stdout={}",
                output.stdout.trim()
            ))
        })?
    } else {
        Value::Null
    };
    if json {
        print_json(&json!({
            "host": host,
            "service": name,
            "image_refresh": image_refresh,
            "unit_repointed": followed,
            "version": installed.version,
            "sha256": installed.sha256,
            "status": if already_active { "already_installed" } else { "updated" },
            "effective": if refresh_image { "running image verified" } else { "on next restart" },
        }))?;
    } else if refresh_image {
        println!(
            "{host}: {name} -> {} (running image verified)",
            installed.version
        );
    } else {
        println!(
            "{host}: {name} -> {} (takes effect on the next restart)",
            installed.version
        );
    }
    Ok(())
}

struct ServiceReleaseOptions<'a> {
    name: &'a str,
    host: &'a str,
    product: &'a str,
    version: &'a str,
    readiness_url: Option<&'a str>,
    readiness_timeout_seconds: u64,
    reload_unit: bool,
    require_release_version: bool,
    supersede_unit: Option<&'a str>,
    supersede_same_label_user: bool,
    json: bool,
    emit: bool,
}

pub(crate) async fn release_pipeline_product(
    name: &str,
    host: &str,
    product: &str,
    version: &str,
    readiness_url: &str,
    readiness_timeout_seconds: u64,
) -> Result<(), CmdError> {
    release(ServiceReleaseOptions {
        name,
        host,
        product,
        version,
        readiness_url: Some(readiness_url),
        readiness_timeout_seconds,
        reload_unit: false,
        require_release_version: true,
        supersede_unit: None,
        supersede_same_label_user: true,
        json: false,
        emit: false,
    })
    .await
}

#[derive(Default, Deserialize)]
struct ObservedServiceRelease {
    active_version: Option<String>,
    active_sha256: Option<String>,
}

struct ServiceReleaseBundle {
    artifact: crate::release_control::ReleaseArtifactRef,
    archive: Vec<u8>,
    rollout_generation: u64,
    previous_version: Option<String>,
    previous_sha256: Option<String>,
}

async fn service_release_bundle(
    options: &ServiceReleaseOptions<'_>,
    target: &targets::ComputeTarget,
    declared: &ManagedService,
) -> Result<ServiceReleaseBundle, CmdError> {
    let document = registry::fetch_document().await?;
    let control = crate::release_control::control(&document)?
        .ok_or_else(|| CmdError::click("registry.release_control is not configured"))?;
    let policy = control.products.get(options.product).ok_or_else(|| {
        CmdError::click(format!(
            "registry.release_control declares no product {:?}",
            options.product
        ))
    })?;
    let target_policy = policy.targets.get(options.host).ok_or_else(|| {
        CmdError::click(format!(
            "product {:?} has no release target {:?}",
            options.product, options.host
        ))
    })?;
    let exact_legacy_unit = target_policy
        .legacy_launchd_label
        .as_deref()
        .is_some_and(|label| label == declared.unit_id());
    if policy.service != options.name && policy.service != declared.name && !exact_legacy_unit {
        return Err(CmdError::click(format!(
            "product {:?} releases service {:?}, not unit {:?}",
            options.product,
            policy.service,
            declared.unit_id()
        )));
    }
    if target_policy.platform != target.release_platform {
        return Err(CmdError::click(format!(
            "release target platform {:?} disagrees with host platform {:?}",
            target_policy.platform, target.release_platform
        )));
    }
    let desired = policy.desired.as_ref().ok_or_else(|| {
        CmdError::click(format!(
            "product {:?} has no desired release",
            options.product
        ))
    })?;
    if desired.version != options.version {
        return Err(CmdError::click(format!(
            "product {:?} desires {}, not {}",
            options.product, desired.version, options.version
        )));
    }
    let artifact = desired
        .artifacts
        .get(&target_policy.platform)
        .cloned()
        .ok_or_else(|| {
            CmdError::click(format!(
                "desired release has no artifact for {:?}",
                target_policy.platform
            ))
        })?;
    let (_, archive, _) = crate::release_agent::fetch_candidate(
        &control,
        options.product,
        desired,
        &artifact,
        policy,
        target_policy,
    )
    .await
    .map_err(CmdError::click)?;

    let observed_uri = crate::release_agent::release_status_uri(options.product, options.host);
    let observed = match crate::cli::storage::fetch_object(&observed_uri).await {
        Ok(bytes) => serde_json::from_slice::<ObservedServiceRelease>(&bytes).unwrap_or_default(),
        Err(_) => ObservedServiceRelease::default(),
    };
    let previous_version = observed.active_version.or_else(|| {
        policy
            .previous
            .as_ref()
            .map(|release| release.version.clone())
    });
    let previous_sha256 = observed.active_sha256.or_else(|| {
        policy.previous.as_ref().and_then(|release| {
            release
                .artifacts
                .get(&target_policy.platform)
                .map(|artifact| artifact.artifact_sha256.clone())
        })
    });
    Ok(ServiceReleaseBundle {
        artifact,
        archive,
        rollout_generation: desired.rollout_generation,
        previous_version,
        previous_sha256,
    })
}

fn stage_service_release_archive(
    product: &str,
    version: &str,
    platform: &str,
    archive: &[u8],
) -> Result<std::path::PathBuf, CmdError> {
    let root = crate::config_file::expand_tilde("~")
        .join(".stado/work/service-release")
        .join(product)
        .join(version);
    std::fs::create_dir_all(&root)?;
    let path = root.join(format!("{platform}.tar.gz"));
    std::fs::write(&path, archive)?;
    Ok(path)
}

async fn current_service_version(
    target: &targets::ComputeTarget,
    directory: &str,
    runner: &crate::deploy::Runner,
) -> Result<String, CmdError> {
    let script = format!(
        "set -euo pipefail\nname={}\nroot=\"$HOME/.stado/services/$name\"\n\
         target=$(/usr/bin/readlink \"$root/current\")\n\
         /usr/bin/basename \"$target\"",
        crate::deploy::shlex_quote(directory),
    );
    let output = host_channel::run_script(target, &script, runner)
        .await
        .map_err(click)?;
    if !output.ok() {
        return Err(CmdError::click(host_channel::last_error_line(
            &output,
            "current service version is unreadable",
        )));
    }
    let version = output.stdout.trim();
    if version.is_empty() {
        return Err(CmdError::click("current service version is empty"));
    }
    Ok(version.to_string())
}

fn validate_readiness_url(url: &str) -> Result<(), CmdError> {
    if ["http://127.0.0.1:", "http://localhost:", "http://[::1]:"]
        .iter()
        .any(|prefix| url.starts_with(prefix))
        && !url.chars().any(char::is_whitespace)
    {
        return Ok(());
    }
    Err(CmdError::usage(
        "--readiness-url must be a whitespace-free loopback HTTP URL",
    ))
}

/// The host-side readiness probe, as a script, separated from running it so
/// its refusals can be exercised directly.
///
/// Three distinct failures used to share one sentence. A timeout appended
/// `did not report releaseVersion or build.version <expected>` whenever a
/// version was required, including when `curl` never once succeeded — so a
/// candidate whose HTTP server never bound its port was reported as one that
/// answered without a version field. Three weles-worker rollouts were rolled
/// back on that sentence on 2026-09-02 while the real fault was
/// `EADDRINUSE` on the service's port, and it sent the next reader hunting a
/// field contract that was satisfiable the whole time. The three repairs are
/// different — start the service, publish the release the gate asked for, or
/// teach the service to report its identity — so the sentence names which
/// one is owed:
///
/// - the URL never answered,
/// - it answered and reported a value that is not the expected one,
/// - it answered with neither field present.
///
/// `expected` empty means no version is required, and then the first answer
/// is readiness, so a timeout in that mode can only be the first case.
fn readiness_probe_script(
    url: &str,
    expected_release_version: Option<&str>,
    timeout_seconds: u64,
) -> String {
    format!(
        "set -euo pipefail\nurl={url}\nexpected={expected}\n\
         deadline=$((SECONDS + {timeout}))\n\
         answered=no\n\
         reported=\n\
         while [ \"$SECONDS\" -lt \"$deadline\" ]; do\n\
           if body=$(/usr/bin/curl -fsS --max-time 2 \"$url\"); then\n\
             answered=yes\n\
             reported=\n\
             if [ -n \"$expected\" ]; then\n\
               reported=\"$(printf '%s' \"$body\" | /usr/bin/plutil -extract releaseVersion raw -o - - 2>/dev/null)\" || reported=\n\
               if [ -z \"$reported\" ]; then\n\
                 reported=\"$(printf '%s' \"$body\" | /usr/bin/plutil -extract build.version raw -o - - 2>/dev/null)\" || reported=\n\
               fi\n\
             fi\n\
             if [ -z \"$expected\" ] || [ \"$reported\" = \"$expected\" ]; then\n\
               printf '%s\\n' ready\n\
               exit 0\n\
             fi\n\
           fi\n\
           /bin/sleep 1\n\
         done\n\
         if [ \"$answered\" = no ]; then\n\
           detail=\"readiness timed out after {timeout}s: $url never answered\"\n\
         elif [ -z \"$reported\" ]; then\n\
           detail=\"readiness timed out after {timeout}s: $url answered, and reported neither releaseVersion nor build.version, where $expected was required\"\n\
         else\n\
           detail=\"readiness timed out after {timeout}s: $url answered, and reported $reported, not the required $expected\"\n\
         fi\n\
         printf '%s\\n' \"$detail\" >&2\n\
         exit 1",
        url = crate::deploy::shlex_quote(url),
        expected = crate::deploy::shlex_quote(expected_release_version.unwrap_or_default()),
        timeout = timeout_seconds,
    )
}

async fn wait_for_service_readiness(
    target: &targets::ComputeTarget,
    url: &str,
    expected_release_version: Option<&str>,
    timeout_seconds: u64,
    runner: &crate::deploy::Runner,
) -> Result<(), CmdError> {
    validate_readiness_url(url)?;
    if timeout_seconds == 0 || timeout_seconds > 600 {
        return Err(CmdError::usage(
            "--readiness-timeout-seconds must be between 1 and 600",
        ));
    }
    let script = readiness_probe_script(url, expected_release_version, timeout_seconds);
    let output = host_channel::run_script(target, &script, runner)
        .await
        .map_err(click)?;
    if output.ok() {
        Ok(())
    } else {
        Err(CmdError::click(host_channel::last_error_line(
            &output,
            "readiness failed",
        )))
    }
}

/// Relink the previous release and bring the unit back.
///
/// Takes the release's own [`ServiceReleaseOptions`] rather than restating
/// three of its fields: the sole caller already holds it, and the file
/// carries the same shape for `secret-sync`, `file-sync` and `file-fetch`.
/// Eight loose parameters also put the release quality gate over
/// `clippy::too_many_arguments`, which is denied there, so no product release
/// could be submitted.
async fn rollback_service_release(
    options: &ServiceReleaseOptions<'_>,
    previous: &str,
    target: &targets::ComputeTarget,
    declared: &ManagedService,
    sudo_password: Option<&str>,
    runner: &crate::deploy::Runner,
) -> Result<(), CmdError> {
    update(
        options.name,
        options.host,
        None,
        None,
        Some(previous),
        false,
        false,
    )
    .await?;
    let report = if options.reload_unit {
        service::reload_service_with_password(target, declared, sudo_password, runner).await
    } else {
        service::restart_service_with_password(target, declared, sudo_password, runner).await
    }
    .map_err(click)?;
    if report.succeeded("restarted") {
        Ok(())
    } else {
        Err(CmdError::click(format!(
            "rollback relinked {previous}, but restart failed: {}",
            report.failure()
        )))
    }
}

/// Move the service directory's immutable source with a successful product
/// release. Without this write the release runner advances `current` on the
/// host while `service converge` keeps the old artifact in the declaration and
/// can later put that old release back.
async fn record_released_service_source(
    options: &ServiceReleaseOptions<'_>,
    artifact: &crate::release_control::ReleaseArtifactRef,
) -> Result<(), CmdError> {
    let source_ref = artifact
        .archive_uri
        .strip_suffix("/release.tar.gz")
        .ok_or_else(|| {
            CmdError::click(format!(
                "release archive URI {:?} has no service artifact coordinate",
                artifact.archive_uri
            ))
        })?;
    // The decision read: whether the pin has to move at all. A directory that
    // already names this artifact is not written, so a release that changed
    // nothing does not spend a compare-and-swap or advance the counter.
    let document = registry::fetch_document().await?;
    let logical = released_route(&document, options.name)?;
    if !source_pin_moves(&document, &logical, source_ref, &artifact.artifact_sha256)? {
        return Ok(());
    }
    // Pure: the artifact coordinate is already fixed by the release that just
    // succeeded, so pinning it is a function of the document it is pinned in.
    // `advance_generation` runs INSIDE the transform, on the document that
    // round read, because the counter it derives belongs to that document.
    registry::commit_document(|current| {
        let mut document = current.clone();
        let logical = released_route(&document, options.name)?;
        if !source_pin_moves(&document, &logical, source_ref, &artifact.artifact_sha256)? {
            // Another writer pinned the same artifact first. Its document is
            // already the answer, so this round republishes it verbatim rather
            // than advancing a counter for a change nobody made.
            return Ok(document);
        }
        let source = document
            .get_mut("service_directory")
            .and_then(|directory| directory.get_mut("services"))
            .and_then(|services| services.get_mut(&logical))
            .and_then(Value::as_object_mut)
            .and_then(|entry| entry.get_mut("declaration"))
            .and_then(Value::as_object_mut)
            .and_then(|declaration| declaration.get_mut("source"))
            .and_then(Value::as_object_mut)
            .ok_or_else(|| CmdError::click("release service route disappeared"))?;
        source.insert("artifact".to_string(), json!(source_ref));
        source.insert(
            "sha256".to_string(),
            json!(artifact.artifact_sha256.as_str()),
        );
        crate::service_resolution::advance_generation(&mut document).map_err(CmdError::click)?;
        Ok(document)
    })
    .await?;
    Ok(())
}

/// The one directory route that carries this managed service. Ambiguity is
/// refused rather than guessed: pinning the wrong route's artifact is how a
/// release lands on a service nobody released.
fn released_route(document: &Value, name: &str) -> Result<String, CmdError> {
    let services = document
        .get("service_directory")
        .and_then(|directory| directory.get("services"))
        .and_then(Value::as_object)
        .ok_or_else(|| CmdError::click("registry carries no service directory"))?;
    let matching = services
        .iter()
        .filter(|(logical, entry)| {
            logical.as_str() == name
                || entry.get("managed_service").and_then(Value::as_str) == Some(name)
                || placement_declares_unit(document, entry, logical, name)
        })
        .map(|(logical, _)| logical.clone())
        .collect::<Vec<_>>();
    match matching.as_slice() {
        [logical] => Ok(logical.clone()),
        [] => Err(CmdError::click(format!(
            "service directory carries no route for managed service {name:?}"
        ))),
        several => Err(CmdError::click(format!(
            "managed service {name:?} is shared by {} directory routes ({}); refusing to \
             change an ambiguous declaration",
            several.len(),
            several.join(", ")
        ))),
    }
}

/// Whether a placement-backed route's profile installs `unit` for this service.
///
/// A placement-backed route MUST leave `managed_service` absent - the schema
/// refuses it, because the unit is declared once per host inside the profile.
/// Reading only the absent field made every such service unreachable from a
/// unit name: on 2026-09-05 `service release com.wisent.always-on.brama` moved
/// `current` to the new digest and then failed with "carries no route", so the
/// host ran one release while the directory still described another.
fn placement_declares_unit(document: &Value, entry: &Value, logical: &str, unit: &str) -> bool {
    let Some(profile_name) = entry.get("placement_profile").and_then(Value::as_str) else {
        return false;
    };
    document
        .get("placement_profiles")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|profile| profile.get("name").and_then(Value::as_str) == Some(profile_name))
        .filter_map(|profile| profile.get("hosts").and_then(Value::as_object))
        .flat_map(|hosts| hosts.values())
        .filter_map(|host| host.get("units").and_then(Value::as_object))
        .filter_map(|units| units.get(logical))
        .filter_map(|declared| declared.get("unit").and_then(Value::as_str))
        .any(|declared| declared == unit)
}

/// Whether pinning `artifact`/`sha256` on this route would change anything.
///
/// A route that declares no deployable source has nothing to pin: its version
/// is delivered by the release plane (`release_control`) and the directory
/// carries only its address. That is not a failure of the release that just
/// landed, and treating it as one aborted the command after `current` had
/// already moved.
fn source_pin_moves(
    document: &Value,
    logical: &str,
    artifact: &str,
    sha256: &str,
) -> Result<bool, CmdError> {
    let Some(source) = document
        .get("service_directory")
        .and_then(|directory| directory.get("services"))
        .and_then(|services| services.get(logical))
        .and_then(|entry| entry.get("declaration"))
        .and_then(|declaration| declaration.get("source"))
        .and_then(Value::as_object)
    else {
        return Ok(false);
    };
    Ok(
        source.get("artifact").and_then(Value::as_str) != Some(artifact)
            || source.get("sha256").and_then(Value::as_str) != Some(sha256),
    )
}

async fn release(options: ServiceReleaseOptions<'_>) -> Result<(), CmdError> {
    let target = host_channel::canonical_target(options.host)
        .await
        .map_err(click)?;
    let mut services = declared_matching(options.name, Some(options.host)).await?;
    let Some(current) = services.first() else {
        return Err(CmdError::click(format!(
            "{} does not manage {}; deploy it first",
            options.host, options.name
        )));
    };
    if service::requires_daemon_domain(&target)
        && UnitDomain::from_path(&current.path).is_per_login()
    {
        let reason = format!(
            "release {} {} requires an always-on system service",
            options.product, options.version
        );
        ensure(EnsureOptions {
            name: options.name,
            host: options.host,
            from: None,
            args: &[],
            env: &[],
            reason: &reason,
            as_daemon: true,
            as_launch_agent: false,
            as_json: false,
        })
        .await?;
        services = declared_matching(options.name, Some(options.host)).await?;
    }
    let Some(declared) = services.first() else {
        return Err(CmdError::click(format!(
            "{} lost the managed {} declaration during domain convergence",
            options.host, options.name
        )));
    };
    let automatic_supersede_unit = (options.supersede_same_label_user
        && UnitDomain::from_path(&declared.path) == UnitDomain::System)
        .then(|| declared.unit_id().to_string());
    let requested_supersede_unit = options
        .supersede_unit
        .or(automatic_supersede_unit.as_deref());
    if options.require_release_version && options.readiness_url.is_none() {
        return Err(CmdError::usage(
            "--require-release-version requires --readiness-url",
        ));
    }
    if options.reload_unit && !UnitDomain::from_path(&declared.path).requires_privileged_bootstrap()
    {
        return Err(CmdError::usage(
            "--reload-unit is only needed for a system LaunchDaemon",
        ));
    }
    let runner = production_runner();
    let supersede_unit = if let Some(label) = requested_supersede_unit {
        // One launchd label may exist in both the system and user domains.
        // A managed system daemon superseding its same-named legacy
        // LaunchAgent is safe because every operation below is explicitly
        // scoped: the legacy bootout/restore/delete uses the user domain and
        // the managed restart uses the daemon path. Keep rejecting the same
        // name everywhere else, where the two references would identify one
        // unit rather than the migration pair.
        if label == declared.unit_id()
            && !UnitDomain::from_path(&declared.path).requires_privileged_bootstrap()
        {
            return Err(CmdError::usage(
                "--supersede-unit may match the managed unit only when that unit is a system \
                 LaunchDaemon replacing its same-named legacy user LaunchAgent",
            ));
        }
        let present = if options.supersede_unit.is_some() {
            service::check_user_launchagent(&target, label, &runner)
                .await
                .map_err(click)?;
            true
        } else {
            service::restorable_user_launchagent_exists(&target, label, &runner)
                .await
                .map_err(click)?
        };
        present.then_some(label)
    } else {
        None
    };
    let shown = service::show_service(&target, declared, &runner)
        .await
        .map_err(click)?;
    let program = shown.detail.trim();
    let directory = program
        .split("/services/")
        .nth(usize::from(true))
        .and_then(|rest| rest.split('/').next())
        .filter(|segment| !segment.is_empty())
        .ok_or_else(|| {
            CmdError::click(format!(
                "{}: {} runs {program:?}, which is not under a managed services directory",
                options.host, options.name
            ))
        })?;
    let sudo_password = if UnitDomain::from_path(&declared.path).requires_privileged_bootstrap() {
        host_sudo_password(&target).await?
    } else {
        None
    };
    let bundle = service_release_bundle(&options, &target, declared).await?;
    let archive_path = stage_service_release_archive(
        options.product,
        options.version,
        &target.release_platform,
        &bundle.archive,
    )?;
    let previous_directory = current_service_version(&target, directory, &runner).await?;

    let superseded_was_running = if let Some(label) = supersede_unit {
        // `--supersede-unit` names a user LaunchAgent by definition, and the
        // unscoped call would have taken out a system job of the same label
        // first, which is the opposite of superseding.
        let (state, detail) =
            service::bootout_label(&target, label, service::BootoutScope::User, &runner)
                .await
                .map_err(click)?;
        match state.as_str() {
            "booted_out" => true,
            "absent" => false,
            _ => {
                return Err(CmdError::click(format!(
                    "could not supersede user LaunchAgent {label}: {detail}"
                )))
            }
        }
    } else {
        false
    };
    if let Err(error) = update(
        options.name,
        options.host,
        None,
        archive_path.to_str(),
        None,
        false,
        false,
    )
    .await
    {
        if superseded_was_running {
            if let Some(label) = supersede_unit {
                service::restore_user_launchagent(&target, label, &runner)
                    .await
                    .map_err(click)?;
            }
        }
        return Err(error);
    }
    let installed_directory = current_service_version(&target, directory, &runner).await?;
    let restart = if options.reload_unit {
        service::reload_service_with_password(&target, declared, sudo_password.as_deref(), &runner)
            .await
    } else {
        service::restart_service_with_password(&target, declared, sudo_password.as_deref(), &runner)
            .await
    }
    .map_err(click);
    let activation = match restart {
        Ok(report) if report.succeeded("restarted") => {
            if let Some(url) = options.readiness_url {
                let expected = options.require_release_version.then_some(options.version);
                wait_for_service_readiness(
                    &target,
                    url,
                    expected,
                    options.readiness_timeout_seconds,
                    &runner,
                )
                .await
            } else {
                Ok(())
            }
        }
        Ok(report) => Err(CmdError::click(format!(
            "restart failed: {}",
            report.failure()
        ))),
        Err(error) => Err(error),
    };
    if let Err(error) = activation {
        let rollback = rollback_service_release(
            &options,
            &previous_directory,
            &target,
            declared,
            sudo_password.as_deref(),
            &runner,
        )
        .await;
        let legacy_restore = if superseded_was_running {
            if let Some(label) = supersede_unit {
                service::restore_user_launchagent(&target, label, &runner)
                    .await
                    .map_err(click)
            } else {
                Ok(())
            }
        } else {
            Ok(())
        };
        return match (rollback, legacy_restore) {
            (Ok(()), Ok(())) => {
                crate::release_agent::publish_service_release_status(
                    options.product,
                    options.host,
                    bundle.rollout_generation,
                    crate::release_agent::RolloutPhase::RolledBack,
                    bundle.previous_version.as_deref(),
                    bundle.previous_sha256.as_deref(),
                    Some(options.version),
                    "service readiness failed; previous release and legacy unit restored",
                )
                .await
                .map_err(CmdError::click)?;
                Err(CmdError::click(format!(
                    "{error}; rolled back to {previous_directory} and restored the prior unit"
                )))
            }
            (Err(rollback_error), Ok(())) => Err(CmdError::click(format!(
                "{error}; rollback to {previous_directory} also failed: {rollback_error}"
            ))),
            (Ok(()), Err(legacy_error)) => Err(CmdError::click(format!(
                "{error}; managed release rolled back, but the legacy unit could not be restored: {legacy_error}"
            ))),
            (Err(rollback_error), Err(legacy_error)) => Err(CmdError::click(format!(
                "{error}; managed rollback failed: {rollback_error}; legacy restore failed: {legacy_error}"
            ))),
        };
    }
    if let Some(label) = supersede_unit {
        service::delete_user_launchagent(&target, label, &runner)
            .await
            .map_err(click)?;
    }

    crate::release_agent::publish_service_release_status(
        options.product,
        options.host,
        bundle.rollout_generation,
        crate::release_agent::RolloutPhase::Committed,
        Some(options.version),
        Some(&bundle.artifact.artifact_sha256),
        bundle.previous_version.as_deref(),
        if supersede_unit.is_some() {
            "service readiness passed; superseded user LaunchAgent removed"
        } else if options.readiness_url.is_some() {
            "service restart and readiness passed"
        } else {
            "service restarted; no readiness endpoint was requested"
        },
    )
    .await
    .map_err(CmdError::click)?;
    record_released_service_source(&options, &bundle.artifact).await?;
    let report = json!({
        "host": options.host,
        "service": options.name,
        "product": options.product,
        "previous_version": bundle.previous_version,
        "version": options.version,
        "artifact_sha256": bundle.artifact.artifact_sha256,
        "artifact_directory": installed_directory,
        "status": "released",
        "readiness": if options.readiness_url.is_some() { "passed" } else { "unit-running" },
        "superseded_unit": supersede_unit,
    });
    if options.emit {
        if options.json {
            print_json(&report)?;
        } else {
            let readiness = if options.readiness_url.is_some() {
                "restart and readiness passed"
            } else {
                "unit restarted; readiness was not requested"
            };
            println!(
                "{}: {} released {} {} ({readiness})",
                options.host, options.name, options.product, options.version
            );
        }
    }
    Ok(())
}

async fn show(name: &str, host: Option<&str>, json: bool) -> Result<(), CmdError> {
    let services = declared_matching(name, host).await?;
    let runner = production_runner();
    let mut payload: Vec<Value> = Vec::new();
    let mut cells: Vec<Vec<String>> = Vec::new();

    for declared in &services {
        let target = host_channel::canonical_target(&declared.host)
            .await
            .map_err(click)?;
        let report = service::show_service(&target, declared, &runner)
            .await
            .map_err(click)?;
        cells.push(vec![
            declared.host.clone(),
            declared.unit_id().to_string(),
            dash(&report.detail),
        ]);
        let mut entry = report.to_json();
        entry["host"] = Value::from(declared.host.clone());
        payload.push(entry);
    }

    if json {
        print_json(&Value::Array(payload))?;
    } else {
        table::print(&["HOST", "UNIT", "RUNS"], &cells);
    }
    Ok(())
}

async fn stop(
    name: &str,
    host: Option<&str>,
    listener_url: Option<&str>,
    json: bool,
) -> Result<(), CmdError> {
    let services = declared_matching(name, host).await?;
    let runner = production_runner();
    let mut payload: Vec<Value> = Vec::new();
    let mut cells: Vec<Vec<String>> = Vec::new();
    let mut failures: Vec<String> = Vec::new();

    for declared in &services {
        let target = host_channel::canonical_target(&declared.host)
            .await
            .map_err(click)?;
        let sudo_password = if UnitDomain::from_path(&declared.path).requires_privileged_bootstrap()
        {
            host_sudo_password(&target).await?
        } else {
            None
        };
        let report = service::stop_service_with_password(
            &target,
            declared,
            sudo_password.as_deref(),
            &runner,
        )
        .await
        .map_err(click)?;
        if let Some(url) = listener_url {
            let listener = service::reset_service_listener(&target, declared, url, &runner)
                .await
                .map_err(click)?;
            if !listener.succeeded("listener_stopped") && !listener.succeeded("listener_absent") {
                failures.push(format!("{}: {}", declared.host, listener.failure()));
            }
        }
        if !report.succeeded("stopped") {
            failures.push(format!("{}: {}", declared.host, report.failure()));
        }
        cells.push(vec![
            declared.host.clone(),
            declared.unit_id().to_string(),
            dash(&report.domain),
            dash(&report.status),
            dash(&report.detail),
        ]);
        let mut entry = report.to_json();
        entry["host"] = Value::from(declared.host.clone());
        payload.push(entry);
    }

    if json {
        print_json(&Value::Array(payload))?;
    } else {
        table::print(&["HOST", "UNIT", "DOMAIN", "STATUS", "DETAIL"], &cells);
    }
    fail_if_any(&failures, "stop")
}

pub(crate) async fn service_secret(item: &str, field: &str) -> Result<String, CmdError> {
    let vault = crate::skarbiec::Client::service_verifier()
        .map_err(|err| CmdError::click(err.to_string()))?;
    // Both callers -- auth-check and secret-sync -- want exactly one field, and
    // asking for the whole item is refused outright by a broker that requires a
    // named field. Ask for what is wanted.
    let stored = vault
        .read_field(item, field)
        .await
        .map_err(|err| CmdError::click(err.to_string()))?;
    stored
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            CmdError::click(format!(
                "Skarbiec item {item:?} has no non-empty string field {field:?}"
            ))
        })
}

struct SecretSyncOptions<'a> {
    name: &'a str,
    host: &'a str,
    item: &'a str,
    field: &'a str,
    variable: &'a str,
    env_file: &'a str,
    restart_after_sync: bool,
    as_json: bool,
}

async fn secret_sync(options: SecretSyncOptions<'_>) -> Result<(), CmdError> {
    let SecretSyncOptions {
        name,
        host,
        item,
        field,
        variable,
        env_file,
        restart_after_sync,
        as_json,
    } = options;
    let secret = service_secret(item, field).await?;

    let services = declared_matching(name, Some(host)).await?;
    let runner = production_runner();
    let mut payload: Vec<Value> = Vec::new();
    let mut cells: Vec<Vec<String>> = Vec::new();
    let mut failures: Vec<String> = Vec::new();

    for declared in &services {
        let target = host_channel::canonical_target(&declared.host)
            .await
            .map_err(click)?;
        let synced =
            service::sync_service_secret(&target, declared, env_file, variable, &secret, &runner)
                .await
                .map_err(click)?;
        let sync_ok = synced.succeeded("secret_synced");
        if !sync_ok {
            failures.push(format!("{}: {}", declared.host, synced.failure()));
        }

        let restarted = if sync_ok && restart_after_sync {
            Some(
                service::restart_service(&target, declared, &runner)
                    .await
                    .map_err(click)?,
            )
        } else {
            None
        };
        if let Some(report) = &restarted {
            if !report.succeeded("restarted") {
                failures.push(format!("{}: {}", declared.host, report.failure()));
            }
        }

        let restart_status = match &restarted {
            Some(report) => dash(&report.status),
            None if restart_after_sync => "skipped".to_string(),
            None => "-".to_string(),
        };
        let detail = restarted
            .as_ref()
            .map(|report| report.detail.as_str())
            .filter(|detail| !detail.is_empty())
            .unwrap_or(&synced.detail);
        cells.push(vec![
            declared.host.clone(),
            declared.unit_id().to_string(),
            dash(&synced.status),
            restart_status,
            dash(detail),
        ]);
        payload.push(json!({
            "host": declared.host,
            "unit": declared.unit_id(),
            "item": item,
            "field": field,
            "variable": variable,
            "env_file": env_file,
            "sync": synced.to_json(),
            "restart": restarted.as_ref().map(|report| report.to_json()),
        }));
    }
    drop(secret);

    if as_json {
        print_json(&Value::Array(payload))?;
    } else {
        table::print(&["HOST", "UNIT", "SYNC", "RESTART", "DETAIL"], &cells);
    }
    fail_if_any(&failures, "secret sync")
}

struct FileSyncOptions<'a> {
    name: &'a str,
    host: &'a str,
    source_file: &'a str,
    target_file: &'a str,
    executable: bool,
    as_json: bool,
}

async fn file_sync(options: FileSyncOptions<'_>) -> Result<(), CmdError> {
    let FileSyncOptions {
        name,
        host,
        source_file,
        target_file,
        executable,
        as_json,
    } = options;
    let source = std::path::Path::new(source_file);
    if !source.is_absolute() {
        return Err(CmdError::click("--source-file must be absolute"));
    }
    let metadata = std::fs::symlink_metadata(source)
        .map_err(|error| CmdError::click(format!("cannot read {source_file}: {error}")))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(CmdError::click(format!(
            "{source_file} must be a regular file, not a symlink"
        )));
    }
    let max_bytes = if executable {
        96 * 1_048_576
    } else {
        1_048_576
    };
    if metadata.len() > max_bytes {
        return Err(CmdError::click(format!(
            "{source_file} exceeds the {} MiB service file limit",
            max_bytes / 1_048_576
        )));
    }
    #[cfg(unix)]
    if !executable && metadata.permissions().mode() & 0o077 != 0 {
        return Err(CmdError::click(format!(
            "{source_file} must be owner-only unless --executable is set"
        )));
    }
    let content = std::fs::read(source)
        .map_err(|error| CmdError::click(format!("cannot read {source_file}: {error}")))?;
    if content.is_empty() {
        return Err(CmdError::click(format!("{source_file} is empty")));
    }
    let mode = if executable { 0o700 } else { 0o600 };

    let services = declared_matching(name, Some(host)).await?;
    let runner = production_runner();
    let mut payload = Vec::new();
    let mut cells = Vec::new();
    let mut failures = Vec::new();

    for declared in &services {
        let target = host_channel::canonical_target(&declared.host)
            .await
            .map_err(click)?;
        let synced = service::sync_service_file(&target, target_file, &content, mode, &runner)
            .await
            .map_err(click)?;
        if !synced.succeeded("file_synced") {
            failures.push(format!("{}: {}", declared.host, synced.failure()));
        }
        cells.push(vec![
            declared.host.clone(),
            declared.unit_id().to_string(),
            dash(&synced.status),
            dash(&synced.detail),
        ]);
        payload.push(json!({
            "host": declared.host,
            "unit": declared.unit_id(),
            "target_file": target_file,
            "mode": format!("{mode:04o}"),
            "sync": synced.to_json(),
        }));
    }

    if as_json {
        print_json(&Value::Array(payload))?;
    } else {
        table::print(&["HOST", "UNIT", "SYNC", "DETAIL"], &cells);
    }
    fail_if_any(&failures, "file sync")
}

struct FileFetchOptions<'a> {
    name: &'a str,
    host: &'a str,
    source_file: &'a str,
    dest_file: Option<&'a str>,
    as_json: bool,
}

/// `service file-fetch`: the byte-exact read `env-show` deliberately is not.
///
/// The write happens only after both digests agree, and the destination is
/// replaced by a rename from a sibling temporary file. A partially written
/// destination is the one outcome that would make this command worse than the
/// hand copy it replaces: an operator would commit it.
async fn file_fetch(options: FileFetchOptions<'_>) -> Result<(), CmdError> {
    let FileFetchOptions {
        name,
        host,
        source_file,
        dest_file,
        as_json,
    } = options;
    if let Some(destination) = dest_file {
        if !std::path::Path::new(destination).is_absolute() {
            return Err(CmdError::click("--dest-file must be absolute"));
        }
    }
    let services = declared_matching(name, Some(host)).await?;
    let runner = production_runner();
    let mut payload = Vec::new();
    let mut cells = Vec::new();
    let mut failures = Vec::new();

    for declared in &services {
        let target = host_channel::canonical_target(&declared.host)
            .await
            .map_err(click)?;
        let fetched = service_file_fetch::fetch_file(&target, source_file, &runner)
            .await
            .map_err(click)?;
        if let Some(failure) = fetched.failure(&declared.host) {
            failures.push(failure);
        }
        let written = match dest_file {
            Some(destination) if fetched.ok() => {
                write_owner_only(destination, &fetched.content)?;
                destination
            }
            _ => "-",
        };
        cells.push(vec![
            declared.host.clone(),
            declared.unit_id().to_string(),
            dash(&fetched.report.file_state),
            fetched.report.bytes.to_string(),
            dash(&fetched.report.mode),
            fetched.integrity.to_string(),
            fetched.local_digest.clone(),
            written.to_string(),
        ]);
        let mut object = fetched.to_report(&target, declared.unit_id());
        object.insert("dest_file".to_string(), json!(written));
        payload.push(Value::Object(object));
    }

    if as_json {
        print_json(&Value::Array(payload))?;
    } else {
        table::print(
            &[
                "HOST",
                "UNIT",
                "FILE",
                "BYTES",
                "MODE",
                "INTEGRITY",
                "SHA256",
                "WROTE",
            ],
            &cells,
        );
    }
    fail_if_any(&failures, "file fetch")
}

/// Replace one local path with these bytes, owner-only, through a rename.
fn write_owner_only(destination: &str, content: &[u8]) -> Result<(), CmdError> {
    let path = std::path::Path::new(destination);
    let staged = path.with_extension("stado-file-fetch");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            CmdError::click(format!("cannot create {}: {error}", parent.display()))
        })?;
    }
    std::fs::write(&staged, content)
        .map_err(|error| CmdError::click(format!("cannot stage {destination}: {error}")))?;
    #[cfg(unix)]
    std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o600))
        .map_err(|error| CmdError::click(format!("cannot protect {destination}: {error}")))?;
    std::fs::rename(&staged, path)
        .map_err(|error| CmdError::click(format!("cannot install {destination}: {error}")))
}

fn validate_env_key(key: &str) -> Result<(), CmdError> {
    if key.is_empty()
        || key
            .as_bytes()
            .first()
            .is_some_and(|byte| byte.is_ascii_digit())
        || !key.chars().all(|character| {
            character.is_ascii_uppercase() || character.is_ascii_digit() || character == '_'
        })
    {
        return Err(CmdError::click(
            "--key must be an uppercase environment variable name",
        ));
    }
    Ok(())
}

struct EnvSetOptions<'a> {
    name: &'a str,
    host: &'a str,
    key: &'a str,
    env_file: &'a str,
    value_file: &'a str,
    as_json: bool,
}

/// What the env file said about one key immediately after it was written.
struct ReadBack {
    /// [`service_env_file::EXPECT_MATCHED`], `_DIFFERS`, `_ABSENT` or
    /// `_UNVERIFIED`.
    state: &'static str,
    /// The value the file actually holds now, when the host was willing to
    /// show it. `None` for a withheld value, whose length is still reported.
    effective: Option<String>,
    /// How long that value is, shown or withheld.
    chars: u32,
    /// `name (path)` of the forward marker that holds exactly the value which
    /// replaced ours, when one does. This is the declaration to correct.
    marker: Option<String>,
}

impl ReadBack {
    /// The `EFFECTIVE` column: the value, or its length when it is withheld.
    fn effective_cell(&self) -> String {
        match &self.effective {
            Some(value) => value.clone(),
            None if self.state == service_env_file::EXPECT_MATCHED => "-".to_string(),
            None => format!("<withheld, {} chars>", self.chars),
        }
    }

    /// The refusal for a write that did not survive, or `None` when it did.
    ///
    /// It names the marker whenever one holds exactly what came back, because
    /// the operator's next move is to correct that declaration, not to write
    /// this file again — which is what happened twice before this check
    /// existed. With no marker to name it points at the command that
    /// enumerates every unit that could be the writer, rather than shrugging.
    fn failure(&self, host: &str, key: &str) -> Option<String> {
        let observed = match &self.effective {
            Some(value) => format!("{value:?}"),
            None => format!("a withheld {}-character value", self.chars),
        };
        match self.state {
            service_env_file::EXPECT_MATCHED => None,
            service_env_file::EXPECT_DIFFERS => Some(match &self.marker {
                Some(marker) => format!(
                    "{host}: {key} was replaced after the write and now holds {observed}, \
                     which is exactly what the forward marker {marker} declares — correct \
                     that marker, not this file"
                ),
                None => format!(
                    "{host}: {key} was replaced after the write and now holds {observed}; \
                     something on the host owns this key. `stado service list --undeclared` \
                     names every unit that could"
                ),
            }),
            service_env_file::EXPECT_ABSENT => Some(format!(
                "{host}: {key} is assigned nowhere in the file after the write; something \
                 on the host removed it"
            )),
            _ => Some(format!(
                "{host}: the write reported success and could not be read back, so whether \
                 {key} survived is unknown"
            )),
        }
    }
}

/// Read one key back through the channel that just wrote it.
///
/// The comparison happens on the host against the same unquoting a shell would
/// apply, so `KEY='http://127.0.0.1:8895'` and `KEY=http://127.0.0.1:8895`
/// are one value and a secret is verified exactly without its value returning.
/// The forward markers are collected only when the write did not survive.
async fn verify_env_write(
    target: &targets::ComputeTarget,
    env_file: &str,
    key: &str,
    value: &str,
    runner: &crate::deploy::Runner,
) -> Result<ReadBack, CmdError> {
    let request = service_env_file::EnvFileRequest {
        env_path: env_file,
        reveal: None,
        expect: Some((key, value)),
    };
    let report = service_env_file::read_env_file(target, &request, runner)
        .await
        .map_err(click)?;
    let state = match service_env_file::expectation(&report) {
        service_env_file::EXPECT_MATCHED => service_env_file::EXPECT_MATCHED,
        service_env_file::EXPECT_DIFFERS => service_env_file::EXPECT_DIFFERS,
        service_env_file::EXPECT_ABSENT => service_env_file::EXPECT_ABSENT,
        _ => service_env_file::EXPECT_UNVERIFIED,
    };
    let entry = service_env_file::effective_entry(&report, key);
    let effective = entry.and_then(|entry| {
        (entry.value_state != service_env_file::VALUE_REDACTED)
            .then(|| service_env_file::effective_text(&entry.value).to_string())
    });
    let mut marker = None;
    if state == service_env_file::EXPECT_DIFFERS {
        if let Some(observed) = effective.as_deref() {
            // Best effort: a channel that answered the read and not the
            // inventory must not turn "your write was overwritten" into a
            // failed command. The refusal below stands without attribution.
            if let Ok(markers) = service_env_file::forward_markers(target, runner).await {
                marker = service_env_file::marker_holding(&markers, observed).map(|found| {
                    format!("{} ($HOME/.stado/forwards/{}.url)", found.name, found.name)
                });
            }
        }
    }
    Ok(ReadBack {
        state,
        effective,
        chars: entry.map_or(u32::MIN, |entry| entry.chars),
        marker,
    })
}

async fn verify_unit_env_write(
    target: &targets::ComputeTarget,
    declared: &service::ManagedService,
    key: &str,
    value: &str,
    runner: &crate::deploy::Runner,
) -> Result<ReadBack, CmdError> {
    let unit = service::fetch_unit_file(target, declared, runner)
        .await
        .map_err(click)?;
    let observed = service::parse_systemd_unit(&unit.content)
        .env
        .into_iter()
        .rev()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.replace("%%", "%"));
    let state = match observed.as_deref() {
        Some(found) if found == value => service_env_file::EXPECT_MATCHED,
        Some(_) => service_env_file::EXPECT_DIFFERS,
        None => service_env_file::EXPECT_ABSENT,
    };
    Ok(ReadBack {
        state,
        chars: observed
            .as_ref()
            .map_or(0, |value| value.chars().count() as u32),
        effective: observed.and_then(|value| {
            (service::redact_secret_value(key, &value) != service::REDACTED).then_some(value)
        }),
        marker: None,
    })
}

async fn env_set(options: EnvSetOptions<'_>) -> Result<(), CmdError> {
    let EnvSetOptions {
        name,
        host,
        key,
        env_file,
        value_file,
        as_json,
    } = options;
    validate_env_key(key)?;
    let source = std::path::Path::new(value_file);
    if !source.is_absolute() {
        return Err(CmdError::click("--value-file must be absolute"));
    }
    let metadata = std::fs::symlink_metadata(source)
        .map_err(|error| CmdError::click(format!("cannot read {value_file}: {error}")))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(CmdError::click(format!(
            "{value_file} must be a regular file, not a symlink"
        )));
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(CmdError::click(format!("{value_file} must be owner-only")));
    }
    let value = std::fs::read_to_string(source)
        .map_err(|error| CmdError::click(format!("cannot read {value_file}: {error}")))?;
    let value = value.trim();
    if value.is_empty()
        || value
            .chars()
            .any(|character| matches!(character, '\r' | '\n'))
    {
        return Err(CmdError::click(format!(
            "{value_file} must contain one non-empty value"
        )));
    }

    let services = declared_matching(name, Some(host)).await?;
    let runner = production_runner();
    let mut payload = Vec::new();
    let mut cells = Vec::new();
    let mut failures = Vec::new();

    for declared in &services {
        let target = host_channel::canonical_target(&declared.host)
            .await
            .map_err(click)?;
        let unit_env = service::is_systemd_env_file(declared, env_file);
        let updated = if unit_env {
            service::set_unit_env_key_on_host(
                &target,
                declared,
                env_file,
                key,
                Some(value),
                &runner,
            )
            .await
            .map_err(click)?
        } else {
            service::set_env_key_on_host(&target, env_file, key, value, &runner)
                .await
                .map_err(click)?
        };
        let wrote = updated.succeeded("env_set");
        if !wrote {
            failures.push(format!("{}: {}", declared.host, updated.failure()));
        }
        // Read the key back through the same channel. A writer that cannot see
        // its own write is not a writer, it is a hope: on 2026-08-30 this
        // command reported `env_set` twice for a value a host-side reconciler
        // restored within seconds, and nothing said so.
        let verdict = if wrote {
            let readback = if unit_env {
                verify_unit_env_write(&target, declared, key, value, &runner).await?
            } else {
                verify_env_write(&target, env_file, key, value, &runner).await?
            };
            if let Some(failure) = readback.failure(&declared.host, key) {
                failures.push(failure);
            }
            Some(readback)
        } else {
            None
        };
        let state = verdict
            .as_ref()
            .map_or(service_env_file::EXPECT_UNVERIFIED, |readback| {
                readback.state
            });
        cells.push(vec![
            declared.host.clone(),
            declared.unit_id().to_string(),
            key.to_string(),
            dash(&updated.status),
            state.to_string(),
            verdict
                .as_ref()
                .map_or_else(|| "-".to_string(), ReadBack::effective_cell),
            dash(&updated.detail),
        ]);
        payload.push(json!({
            "host": declared.host,
            "unit": declared.unit_id(),
            "key": key,
            "env_file": env_file,
            "value_file": value_file,
            "update": updated.to_json(),
            "readback": state,
            "effective_value": verdict.as_ref().and_then(|readback| readback.effective.clone()),
            "effective_chars": verdict.as_ref().map(|readback| readback.chars),
            "owning_marker": verdict.as_ref().and_then(|readback| readback.marker.clone()),
        }));
    }

    if as_json {
        print_json(&Value::Array(payload))?;
    } else {
        table::print(
            &[
                "HOST",
                "UNIT",
                "KEY",
                "UPDATE",
                "READBACK",
                "EFFECTIVE",
                "DETAIL",
            ],
            &cells,
        );
    }
    fail_if_any(&failures, "environment update")
}

struct EnvUnsetOptions<'a> {
    name: &'a str,
    host: &'a str,
    key: &'a str,
    env_file: &'a str,
    as_json: bool,
}

async fn env_unset(options: EnvUnsetOptions<'_>) -> Result<(), CmdError> {
    let EnvUnsetOptions {
        name,
        host,
        key,
        env_file,
        as_json,
    } = options;
    validate_env_key(key)?;
    let services = declared_matching(name, Some(host)).await?;
    let runner = production_runner();
    let mut payload = Vec::new();
    let mut cells = Vec::new();
    let mut failures = Vec::new();

    for declared in &services {
        let target = host_channel::canonical_target(&declared.host)
            .await
            .map_err(click)?;
        let updated = if service::is_systemd_env_file(declared, env_file) {
            service::set_unit_env_key_on_host(&target, declared, env_file, key, None, &runner)
                .await
                .map_err(click)?
        } else {
            service::unset_env_key_on_host(&target, env_file, key, &runner)
                .await
                .map_err(click)?
        };
        if !updated.succeeded("env_unset") {
            failures.push(format!("{}: {}", declared.host, updated.failure()));
        }
        cells.push(vec![
            declared.host.clone(),
            declared.unit_id().to_string(),
            key.to_string(),
            dash(&updated.status),
            dash(&updated.detail),
        ]);
        payload.push(json!({
            "host": declared.host,
            "unit": declared.unit_id(),
            "key": key,
            "env_file": env_file,
            "update": updated.to_json(),
        }));
    }

    if as_json {
        print_json(&Value::Array(payload))?;
    } else {
        table::print(&["HOST", "UNIT", "KEY", "UPDATE", "DETAIL"], &cells);
    }
    fail_if_any(&failures, "environment update")
}

struct EnvShowOptions<'a> {
    name: &'a str,
    host: &'a str,
    env_file: &'a str,
    reveal: Option<&'a str>,
    as_json: bool,
}

/// The head every env-file reader prints: which file, how protected, how big.
///
/// The mode is not decoration. This file is the one an operator is about to
/// believe, and a `worker.env` the group can read is a finding that belongs
/// next to its contents rather than in a separate audit nobody runs.
fn print_env_file_head(report: &service_env_file::EnvFileReport) {
    println!(
        "env file: {} ({})",
        dash(&report.path),
        if report.file_state == service_env_file::FILE_READ {
            format!(
                "mode {}, {}, {} bytes",
                dash(&report.mode),
                if report.owner_only {
                    "owner-only"
                } else {
                    "READABLE BEYOND ITS OWNER"
                },
                report.bytes
            )
        } else {
            format!("{}: {}", report.file_state, dash(&report.detail))
        }
    );
}

/// Why a report cannot be believed, or `None` when it can.
///
/// A file that was never opened and a file that holds no assignments are
/// opposite answers, and this command must not exit zero on the first while
/// printing the empty table of the second.
fn env_file_failure(host: &str, report: &service_env_file::EnvFileReport) -> Option<String> {
    if report.file_state != service_env_file::FILE_READ {
        return Some(format!(
            "{host}: {} — {}",
            report.file_state,
            dash(&report.detail)
        ));
    }
    if report.entries_state != service_env_file::ENTRIES_READ {
        return Some(format!(
            "{host}: the file was readable and its assignments were not parsed ({})",
            report.entries_state
        ));
    }
    None
}

async fn env_show(options: EnvShowOptions<'_>) -> Result<(), CmdError> {
    let EnvShowOptions {
        name,
        host,
        env_file,
        reveal,
        as_json,
    } = options;
    if let Some(key) = reveal {
        validate_env_key(key)?;
    }
    let services = declared_matching(name, Some(host)).await?;
    let runner = production_runner();
    let mut payload = Vec::new();
    let mut failures = Vec::new();

    for declared in &services {
        let target = host_channel::canonical_target(&declared.host)
            .await
            .map_err(click)?;
        let request = service_env_file::EnvFileRequest {
            env_path: env_file,
            reveal,
            expect: None,
        };
        let report = service_env_file::read_env_file(&target, &request, &runner)
            .await
            .map_err(click)?;
        if let Some(failure) = env_file_failure(&declared.host, &report) {
            failures.push(failure);
        }
        if as_json {
            payload.push(Value::Object(service_env_file::to_report(
                &target,
                declared.unit_id(),
                &report,
            )));
            continue;
        }

        println!("host:     {}", declared.host);
        println!("unit:     {}", declared.unit_id());
        print_env_file_head(&report);
        let roles = service_env_file::shadowing(&report.entries);
        table::print(
            &[
                "LINE",
                "FORM",
                "KEY",
                "RESOLUTION",
                "VALUE STATE",
                "CHARS",
                "VALUE",
            ],
            &report
                .entries
                .iter()
                .zip(&roles)
                .map(|(entry, role)| {
                    vec![
                        entry.line.to_string(),
                        entry.form.clone(),
                        dash(&entry.key),
                        dash(role),
                        entry.value_state.clone(),
                        entry.chars.to_string(),
                        dash(&entry.value),
                    ]
                })
                .collect::<Vec<Vec<String>>>(),
        );
        if report.entries_seen as usize > report.entries.len() {
            println!(
                "entries: {} of {} shown — the rest were cut at this command's cap",
                report.entries.len(),
                report.entries_seen
            );
        }
        // The prime suspect, said in words rather than left for the operator
        // to notice by scanning a KEY column. This is the finding the outage
        // that motivated this command turned on.
        let duplicates = service_env_file::duplicate_keys(&report.entries);
        if duplicates.is_empty() {
            println!("duplicates: none — every key is assigned exactly once");
        } else {
            println!(
                "duplicates: {} — the LAST assignment wins when this file is sourced, so \
                 every row marked {} above is dead text. `env-set` rewrites only lines \
                 spelled KEY=, so an `export KEY=` duplicate survives it.",
                duplicates.join(", "),
                service_env_file::SHADOWED
            );
        }
        let redacted = report
            .entries
            .iter()
            .filter(|entry| entry.value_state == service_env_file::VALUE_REDACTED)
            .count();
        if redacted > usize::MIN {
            println!(
                "redacted: {redacted} value(s) never left the host. Show one with \
                 --reveal KEY."
            );
        }
    }

    if as_json {
        print_json(&Value::Array(payload))?;
    }
    fail_if_any(&failures, "environment read")
}

struct EndpointCheckOptions<'a> {
    name: &'a str,
    host: &'a str,
    env_file: &'a str,
    as_json: bool,
}

async fn endpoint_check(options: EndpointCheckOptions<'_>) -> Result<(), CmdError> {
    let EndpointCheckOptions {
        name,
        host,
        env_file,
        as_json,
    } = options;
    let services = declared_matching(name, Some(host)).await?;
    let runner = production_runner();
    let mut payload = Vec::new();
    let mut failures = Vec::new();

    for declared in &services {
        let target = host_channel::canonical_target(&declared.host)
            .await
            .map_err(click)?;
        let request = service_env_file::EnvFileRequest::read(env_file);
        let report = service_env_file::read_env_file(&target, &request, &runner)
            .await
            .map_err(click)?;
        if let Some(failure) = env_file_failure(&declared.host, &report) {
            failures.push(failure);
        }
        let rows = service_env_file::endpoint_rows(&report);
        let dead: Vec<&str> = rows
            .iter()
            .filter(|row| row.verdict == service_env_file::ENDPOINT_DEAD)
            .map(|row| row.key.as_str())
            .collect();
        if !dead.is_empty() {
            failures.push(format!(
                "{}: nothing is listening where {} points",
                declared.host,
                dead.join(", ")
            ));
        }
        // A check that could not be performed is not a check that passed.
        if report.file_state == service_env_file::FILE_READ
            && report.listeners_state == service_env_file::LISTENERS_FAILED
        {
            failures.push(format!(
                "{}: the socket table could not be read, so no endpoint below was judged",
                declared.host
            ));
        }

        if as_json {
            let mut object = service_env_file::to_report(&target, declared.unit_id(), &report);
            object.insert("listeners_state".to_string(), json!(report.listeners_state));
            object.insert(
                "endpoints".to_string(),
                Value::Array(
                    rows.iter()
                        .map(|row| {
                            json!({
                                "key": row.key,
                                "line": row.line,
                                "declared": row.declared,
                                "port": row.port,
                                "listening": row.verdict,
                                "holders": row.holders,
                            })
                        })
                        .collect(),
                ),
            );
            object.insert("dead_endpoints".to_string(), json!(dead));
            payload.push(Value::Object(object));
            continue;
        }

        println!("host:     {}", declared.host);
        println!("unit:     {}", declared.unit_id());
        print_env_file_head(&report);
        table::print(
            &["KEY", "LINE", "DECLARED", "PORT", "LISTENING", "PROCESS"],
            &rows
                .iter()
                .map(|row| {
                    vec![
                        row.key.clone(),
                        row.line.to_string(),
                        row.declared.clone(),
                        row.port.to_string(),
                        row.verdict.to_string(),
                        dash(&row.holders.join(", ")),
                    ]
                })
                .collect::<Vec<Vec<String>>>(),
        );
        if rows.is_empty() {
            println!(
                "endpoints: none — no effective assignment in this file names a URL or a port"
            );
        }
        if report.listeners_state == service_env_file::LISTENERS_READ_WITHOUT_NAMES {
            // Say why the PROCESS column is thin, where it is being read.
            println!(
                "listeners: {} — lsof was unavailable, so the ports are the kernel's and \
                 the owners are bare pids",
                report.listeners_state
            );
        }
        let shadowed = service_env_file::duplicate_keys(&report.entries);
        if !shadowed.is_empty() {
            println!(
                "duplicates: {} — only the last assignment of each was judged above, \
                 because that is the one the unit runs with",
                shadowed.join(", ")
            );
        }
    }

    if as_json {
        print_json(&Value::Array(payload))?;
    }
    fail_if_any(&failures, "endpoint check")
}

struct ServingOptions<'a> {
    name: &'a str,
    host: &'a str,
    ports: &'a [u16],
    as_json: bool,
}

/// The loopback port the service directory declares for this service on this
/// host, when it declares one.
///
/// This is the fleet's own statement of what the service answers on, which is
/// the only source that distinguishes a port a unit SERVES from one it merely
/// calls. `host_precheck_runner` reads the directory the same way.
///
/// `name` may be either the directory's own key for the service or the launchd
/// label the host declares for it, because this command accepts both and used
/// to resolve the endpoint for neither: `declared_matching` matches the host's
/// labels, this looked the same string up as a directory key, and no argument
/// satisfied both at once. Asked by service name it refused with "is not a
/// registry-managed service"; asked by label, with "the service directory
/// declares no endpoint" -- so the declared port of the service whose
/// declaration was wrong was the one port an operator had to supply by hand.
async fn directory_port(name: &str, host: &str) -> Option<u16> {
    let registry = host_channel::canonical_registry().await.ok()?;
    let key = if registry.service(name).is_some() {
        name
    } else {
        registry.service_named_by_unit(name, host)?
    };
    let endpoint = registry.service(key)?.address_for(host)?;
    url::Url::parse(&endpoint.url).ok()?.port()
}

/// The host's declaration for NAME, accepting the directory's own key for the
/// service as well as the launchd label the host declares.
///
/// [`declared_matching`] matches only the labels a host declares, while
/// `service verify`'s ownership judgement resolves the unit through
/// [`crate::targets::Registry::service_unit`] — which reads `managed_service`
/// and, when a placement profile owns the service instead, that profile's
/// `units` map. The two disagreed: on 2026-09-01 `service verify` judged
/// brama's port by label while `service serving brama` refused with "is not a
/// registry-managed service" on both hosts, because brama carries a
/// `placement_profile` and no `managed_service`. The command #248 points an
/// operator at could not answer for the one service the check was written
/// for. One resolution chain, both commands.
///
/// The label path is tried first so a host that declares a unit under a name
/// the directory also uses keeps resolving to its own declaration.
async fn declared_for_serving(name: &str, host: &str) -> Result<Vec<ManagedService>, CmdError> {
    let refusal = match declared_matching(name, Some(host)).await {
        Ok(found) => return Ok(found),
        Err(refusal) => refusal,
    };
    let Ok(registry) = host_channel::canonical_registry().await else {
        return Err(refusal);
    };
    let Some(unit) = registry.service_unit(name, host) else {
        return Err(refusal);
    };
    declared_matching(unit, Some(host)).await
}

/// `service serving`: the declared unit against the process on its port.
///
/// The ports come from the unit's own env file by the same derivation
/// `endpoint-check` uses, plus any `--port` the operator names. Registry
/// knowledge — whether the label that owns a foreign pid is itself declared —
/// is resolved here rather than on the host, because the registry is this
/// side's document and a host must never be asked to judge its own
/// declaration.
async fn serving(options: ServingOptions<'_>) -> Result<(), CmdError> {
    let ServingOptions {
        name,
        host,
        ports,
        as_json,
    } = options;
    let services = declared_for_serving(name, host).await?;
    let runner = production_runner();
    let target = host_channel::canonical_target(host).await.map_err(click)?;
    // Every label this host declares, so a foreign owner is reported as
    // declared-elsewhere rather than merely foreign. Registry knowledge is
    // this side's; a host is never asked to judge its own declaration.
    let declared_labels: Vec<String> = service::declared_services(&target)
        .iter()
        .map(|found| found.unit_id().to_string())
        .collect();
    let is_declared = |label: &str| declared_labels.iter().any(|known| known == label);

    let mut payload = Vec::new();
    let mut failures = Vec::new();

    for declared in &services {
        let mut wanted: Vec<u16> = ports.to_vec();
        if wanted.is_empty() {
            if let Some(port) = directory_port(name, host).await {
                wanted.push(port);
            }
        }
        if wanted.is_empty() {
            return Err(CmdError::click(format!(
                "the service directory declares no endpoint for {name:?} on {host}, so which \
                 port it must serve is unknown; name it with --port <n>"
            )));
        }
        wanted.truncate(service_serving::MAX_PORTS);

        let report = service_serving::read_serving(
            &target,
            declared.unit_id(),
            &declared.path,
            &wanted,
            &runner,
        )
        .await
        .map_err(click)?;
        let verdicts = service_serving::port_verdicts(&report);
        if let Some(reason) = service_serving::failure(&declared.host, &report, &verdicts) {
            failures.push(reason);
        }

        if as_json {
            payload.push(Value::Object(service_serving::to_report(
                &target,
                &report,
                &verdicts,
                &is_declared,
            )));
            continue;
        }

        println!("host:     {}", declared.host);
        println!("unit:     {}", declared.unit_id());
        println!(
            "launchd:  loaded {}, pid {}",
            report.loaded,
            dash(&report.launchd_pid)
        );
        println!("serving:  {}", service_serving::verdict(&report, &verdicts));
        table::print(
            &["PORT", "VERDICT", "HOLDER", "OWNING UNIT", "DECLARED"],
            &verdicts
                .iter()
                .map(|port| {
                    let owner = port
                        .holders
                        .iter()
                        .find(|holder| holder.owner_state == service_serving::OWNER_RESOLVED)
                        .map(|holder| holder.owner.clone());
                    vec![
                        port.port.to_string(),
                        port.verdict.to_string(),
                        port.holder_cell(),
                        owner.clone().unwrap_or_else(|| "-".to_string()),
                        owner.map_or_else(
                            || "-".to_string(),
                            |label| is_declared(&label).to_string(),
                        ),
                    ]
                })
                .collect::<Vec<Vec<String>>>(),
        );
    }

    if as_json {
        print_json(&Value::Array(payload))?;
    }
    fail_if_any(&failures, "serving check")
}

struct GrantSyncOptions<'a> {
    name: &'a str,
    host: &'a str,
    consumer: &'a str,
    capabilities: &'a [String],
    token_file: &'a str,
    vault_file: &'a str,
    ttl_seconds: u64,
    audience: Option<&'a str>,
    as_json: bool,
}

async fn grant_sync(options: GrantSyncOptions<'_>) -> Result<(), CmdError> {
    let GrantSyncOptions {
        name,
        host,
        consumer,
        capabilities,
        token_file,
        vault_file,
        ttl_seconds,
        audience,
        as_json,
    } = options;
    if ttl_seconds == 0 {
        return Err(CmdError::click("--ttl-seconds must be positive"));
    }
    let capabilities = capabilities.join(",");
    let audience = audience.unwrap_or(consumer);
    let services = declared_matching(name, Some(host)).await?;
    let runner = production_runner();
    let mut payload = Vec::new();
    let mut cells = Vec::new();
    let mut failures = Vec::new();

    for declared in &services {
        let target = host_channel::canonical_target(&declared.host)
            .await
            .map_err(click)?;
        let synced = service::remint_consumer_grant_on_host(
            &target,
            consumer,
            &capabilities,
            token_file,
            vault_file,
            ttl_seconds,
            audience,
            &runner,
        )
        .await
        .map_err(click)?;
        if !synced.succeeded("grant_synced") {
            failures.push(format!("{}: {}", declared.host, synced.failure()));
        }
        cells.push(vec![
            declared.host.clone(),
            declared.unit_id().to_string(),
            consumer.to_string(),
            dash(&synced.status),
            dash(&synced.detail),
        ]);
        payload.push(json!({
            "host": declared.host,
            "unit": declared.unit_id(),
            "consumer": consumer,
            "capabilities": options.capabilities,
            "token_file": token_file,
            "vault_file": vault_file,
            "ttl_seconds": ttl_seconds,
            "audience": audience,
            "sync": synced.to_json(),
        }));
    }

    if as_json {
        print_json(&Value::Array(payload))?;
    } else {
        table::print(&["HOST", "UNIT", "CONSUMER", "SYNC", "DETAIL"], &cells);
    }
    fail_if_any(&failures, "grant sync")
}

struct TokenFileSyncOptions<'a> {
    name: &'a str,
    host: &'a str,
    item: &'a str,
    field: &'a str,
    token_file: &'a str,
    as_json: bool,
}

/// `secret-sync` with a raw file as the destination instead of an `env`
/// assignment.
///
/// Everything before the write is shared with `secret-sync` on purpose: the
/// same isolated service-verifier grant reads the same single field, and the
/// value reaches the host only inside the approved channel's request body.
/// What differs is where it lands -- a file whose entire content is the bearer,
/// which is the only form `WC_STADO_STORAGE_TOKEN_FILE` accepts.
///
/// The three refusals below happen before any host is contacted. An empty
/// `--item` or `--field` would otherwise be sent to Skarbiec as a lookup that
/// cannot match, and an empty `--token-file` would be refused only after the
/// bearer had already been read and put on the wire; a request that cannot
/// succeed should not move a secret at all.
async fn token_file_sync(options: TokenFileSyncOptions<'_>) -> Result<(), CmdError> {
    let TokenFileSyncOptions {
        name,
        host,
        item,
        field,
        token_file,
        as_json,
    } = options;
    if item.trim().is_empty() {
        return Err(CmdError::click("--item must name a Skarbiec item"));
    }
    if field.trim().is_empty() {
        return Err(CmdError::click(
            "--field must name a string field in the Skarbiec item",
        ));
    }
    if token_file.trim().is_empty() {
        return Err(CmdError::click(
            "--token-file must be a file path on the target, absolute or rooted at $HOME",
        ));
    }
    let secret = service_secret(item, field).await?;

    let services = declared_matching(name, Some(host)).await?;
    let runner = production_runner();
    let mut payload = Vec::new();
    let mut cells = Vec::new();
    let mut failures = Vec::new();

    for declared in &services {
        let target = host_channel::canonical_target(&declared.host)
            .await
            .map_err(click)?;
        let synced = service::write_token_file_on_host(&target, token_file, &secret, &runner)
            .await
            .map_err(click)?;
        if !synced.succeeded("token_file_synced") {
            failures.push(format!("{}: {}", declared.host, synced.failure()));
        }
        cells.push(vec![
            declared.host.clone(),
            declared.unit_id().to_string(),
            dash(&synced.status),
            dash(&synced.detail),
        ]);
        payload.push(json!({
            "host": declared.host,
            "unit": declared.unit_id(),
            "item": item,
            "field": field,
            "token_file": token_file,
            "sync": synced.to_json(),
        }));
    }
    drop(secret);

    if as_json {
        print_json(&Value::Array(payload))?;
    } else {
        table::print(&["HOST", "UNIT", "SYNC", "DETAIL"], &cells);
    }
    fail_if_any(&failures, "token file sync")
}

struct AuthCheckOptions<'a> {
    name: &'a str,
    host: &'a str,
    item: Option<&'a str>,
    field: &'a str,
    consumer: Option<&'a str>,
    token_file: Option<&'a str>,
    url: &'a str,
    post_empty_json: bool,
    expect_status: Option<u16>,
    repair: bool,
    take_over_listener: bool,
    variable: Option<&'a str>,
    env_file: Option<&'a str>,
    as_json: bool,
}

async fn auth_check(options: AuthCheckOptions<'_>) -> Result<(), CmdError> {
    let AuthCheckOptions {
        name,
        host,
        item,
        field,
        consumer,
        token_file,
        url,
        post_empty_json,
        expect_status,
        repair,
        take_over_listener,
        variable,
        env_file,
        as_json,
    } = options;
    let repair_target = if repair {
        Some((
            variable.ok_or_else(|| CmdError::click("--repair requires --variable"))?,
            env_file.ok_or_else(|| CmdError::click("--repair requires --env-file"))?,
        ))
    } else {
        None
    };
    if item.is_none() && (variable.is_none() || env_file.is_none()) {
        return Err(CmdError::usage(
            "give --item, or both --variable and --env-file to read the bearer from the unit's own runtime environment",
        ));
    }
    // The bearer source is a property of the invocation, not of the host:
    // either a Skarbiec item read on the host by its own identity, or the
    // exact runtime assignment the unit already runs with. Neither mode
    // brings the secret back over the channel; only the HTTP outcome does.
    // This internal dispatcher preserves the CLI's two mutually exclusive
    // bearer sources; grouping the flags would only duplicate AuthCheckOptions.
    #[allow(clippy::too_many_arguments)]
    async fn check(
        target: &crate::targets::ComputeTarget,
        declared: &ManagedService,
        url: &str,
        item: Option<&str>,
        field: &str,
        consumer: Option<&str>,
        token_file: Option<&str>,
        variable: Option<&str>,
        env_file: Option<&str>,
        post_empty_json: bool,
        expect_status: Option<u16>,
        runner: &crate::deploy::Runner,
    ) -> Result<crate::deploy::service::RemoteReport, CmdError> {
        match (item, variable, env_file) {
            (Some(item), _, _) => service::check_service_item_bearer(
                target,
                declared,
                url,
                item,
                field,
                consumer,
                token_file,
                post_empty_json,
                expect_status,
                runner,
            )
            .await
            .map_err(click),
            (None, Some(variable), Some(env_file)) => service::check_service_env_bearer(
                target,
                declared,
                url,
                env_file,
                variable,
                post_empty_json,
                expect_status,
                runner,
            )
            .await
            .map_err(click),
            _ => unreachable!("usage guard above"),
        }
    }
    let services = declared_matching(name, Some(host)).await?;
    let runner = production_runner();
    let mut payload: Vec<Value> = Vec::new();
    let mut cells: Vec<Vec<String>> = Vec::new();
    let mut failures: Vec<String> = Vec::new();

    for declared in &services {
        let target = host_channel::canonical_target(&declared.host)
            .await
            .map_err(click)?;
        let initial = check(
            &target,
            declared,
            url,
            item,
            field,
            consumer,
            token_file,
            variable,
            env_file,
            post_empty_json,
            expect_status,
            &runner,
        )
        .await?;
        let mut final_report = initial.clone();
        let mut synced = None;
        let mut restarted = None;
        let mut listener_reset = None;

        if !initial.succeeded("auth_ok") {
            if let Some((variable, env_file)) = repair_target {
                let Some(item) = item else {
                    return Err(CmdError::click(
                        "--repair synchronizes from a Skarbiec item; in --env-file mode the runtime file is already the source",
                    ));
                };
                let sync_report = service::sync_service_item_secret(
                    &target, declared, env_file, variable, item, field, &runner,
                )
                .await
                .map_err(click)?;
                let sync_ok = sync_report.succeeded("secret_synced");
                synced = Some(sync_report);
                if sync_ok {
                    let restart_report = service::restart_service(&target, declared, &runner)
                        .await
                        .map_err(click)?;
                    let restart_ok = restart_report.succeeded("restarted");
                    restarted = Some(restart_report);
                    if restart_ok {
                        final_report = check(
                            &target,
                            declared,
                            url,
                            Some(item),
                            field,
                            consumer,
                            token_file,
                            Some(variable),
                            Some(env_file),
                            post_empty_json,
                            expect_status,
                            &runner,
                        )
                        .await?;
                    }
                }
            }
        }

        if repair
            && take_over_listener
            && !final_report.succeeded("auth_ok")
            && synced
                .as_ref()
                .is_some_and(|report| report.succeeded("secret_synced"))
        {
            let reset_report = service::reset_service_listener(&target, declared, url, &runner)
                .await
                .map_err(click)?;
            let reset_ok = reset_report.succeeded("listener_stopped")
                || reset_report.succeeded("listener_absent");
            listener_reset = Some(reset_report);
            if reset_ok {
                let restart_report = service::restart_service(&target, declared, &runner)
                    .await
                    .map_err(click)?;
                let restart_ok = restart_report.succeeded("restarted");
                restarted = Some(restart_report);
                if restart_ok {
                    final_report = check(
                        &target,
                        declared,
                        url,
                        item,
                        field,
                        consumer,
                        token_file,
                        variable,
                        env_file,
                        post_empty_json,
                        expect_status,
                        &runner,
                    )
                    .await?;
                }
            }
        }

        let ok = final_report.succeeded("auth_ok");
        if !ok {
            failures.push(format!("{}: {}", declared.host, final_report.failure()));
        }
        let repair_status = listener_reset
            .as_ref()
            .or(synced.as_ref())
            .map(|report| report.status.as_str())
            .unwrap_or("-");
        cells.push(vec![
            declared.host.clone(),
            declared.unit_id().to_string(),
            dash(&final_report.status),
            dash(repair_status),
            dash(&final_report.detail),
        ]);
        payload.push(json!({
            "host": declared.host,
            "unit": declared.unit_id(),
            "item": item,
            "field": field,
            "url": url,
            "post_empty_json": post_empty_json,
            "expect_status": expect_status,
            "initial": initial.to_json(),
            "sync": synced.as_ref().map(|report| report.to_json()),
            "restart": restarted.as_ref().map(|report| report.to_json()),
            "listener_reset": listener_reset.as_ref().map(|report| report.to_json()),
            "final": final_report.to_json(),
            "ok": ok,
        }));
    }

    if as_json {
        print_json(&Value::Array(payload))?;
    } else {
        table::print(&["HOST", "UNIT", "AUTH", "REPAIR", "DETAIL"], &cells);
    }
    fail_if_any(&failures, "authentication check")
}

/// Report a partial failure after the per-host results have been printed,
/// so the operator sees which hosts worked as well as which did not.
fn fail_if_any(failures: &[String], action: &str) -> Result<(), CmdError> {
    if failures.is_empty() {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "{action} failed on {}",
        failures.join("; ")
    )))
}

// ---------------------------------------------------------------------------
// Adopt / retire / deploy — the registry mutations
// ---------------------------------------------------------------------------

/// Declare a service through the validated conditional write path.
///
/// `commit_document` runs `targets::validate_registry` before it writes, so a
/// declaration that would produce an invalid registry is refused with nothing
/// uploaded. Pure: the record is already decided, and adding it is a function
/// of the document it is added to, so a lost race re-applies it to the newer
/// document. Returns the new generation.
async fn record_declaration(record: &ManagedService) -> Result<String, CmdError> {
    registry::commit_document(|current| {
        let mut document = current.clone();
        // A placeholder left by `service declare` is the declaration, not the
        // unit: deploy replaces it with the real record rather than refusing on
        // a name it put there itself.
        if let Some(services) = document
            .get_mut("targets")
            .and_then(Value::as_array_mut)
            .and_then(|targets| {
                targets
                    .iter_mut()
                    .find(|target| {
                        target.get("name").and_then(Value::as_str) == Some(record.host.as_str())
                    })
                    .and_then(|target| target.get_mut("services"))
                    .and_then(Value::as_array_mut)
            })
        {
            services.retain(|existing| {
                !(existing.get("name").and_then(Value::as_str) == Some(record.name.as_str())
                    && existing.get("declared_only").and_then(Value::as_bool) == Some(true))
            });
        }
        service::add_service(&mut document, record).map_err(click)?;
        Ok(document)
    })
    .await
}

struct OnboardingOptions<'a> {
    name: &'a str,
    host: &'a str,
    product_id: &'a str,
    display_name: &'a str,
    repository: &'a str,
    surfaces: Vec<String>,
    first_success_fact: &'a str,
    onboarding_kind: &'a str,
    status: &'a str,
    as_json: bool,
}

async fn onboarding(options: OnboardingOptions<'_>) -> Result<(), CmdError> {
    // Pure: the onboarding block is a function of the options and of the
    // document it is written into. The record the write produced is what gets
    // rendered, so it is captured from the round that actually landed.
    let recorded = std::cell::RefCell::new(None);
    let generation = registry::commit_document(|current| {
        let mut document = current.clone();
        let record = service::set_service_onboarding(
            &mut document,
            options.host,
            options.name,
            service::OnboardingProduct {
                product_id: options.product_id.to_string(),
                display_name: options.display_name.to_string(),
                repository: options.repository.to_string(),
                surface_kinds: options.surfaces.clone(),
                first_success_fact: options.first_success_fact.to_string(),
                onboarding_kind: options.onboarding_kind.to_string(),
                status: options.status.to_string(),
            },
        )
        .map_err(click)?;
        recorded.replace(Some(record));
        Ok(document)
    })
    .await?;
    let record = recorded
        .into_inner()
        .ok_or_else(|| CmdError::click("onboarding wrote the registry without a record"))?;
    render_mutation("onboarding", &record, &generation, None, options.as_json)
}

async fn adopt(
    unit: &str,
    host: Option<&str>,
    host_heuristic: Option<&str>,
    json: bool,
) -> Result<(), CmdError> {
    let (target, host_heuristic) = resolve_placement(host, host_heuristic).await?;
    let host = target.name.clone();
    let runner = production_runner();
    let report = service::probe_service(&target, unit, &runner)
        .await
        .map_err(click)?;
    if !report.succeeded("probed") {
        return Err(CmdError::click(format!(
            "{host}: could not probe {unit}: {}",
            report.failure()
        )));
    }
    // Adoption claims a unit that is already there. Declaring one that is
    // not present is how a registry starts describing a fleet that does not
    // exist, which is the failure this command was written against.
    if report.file_state != "present" && report.unit_state != "loaded" {
        return Err(CmdError::click(format!(
            "{unit} is not present on {host}: no unit file at {} and the init system does not know it",
            report.path
        )));
    }

    let record =
        service::record_from_report(&host, host_heuristic.as_deref(), unit, &report, &now());
    let generation = record_declaration(&record).await?;
    render_mutation(
        "adopted",
        &record,
        &generation,
        Some(&report.to_json()),
        json,
    )
}

/// Remove the directory half of one declaration. `service declare` writes the
/// target placeholder and `service_directory.services.<name>` in one registry
/// update; retire/remove must drop both in the same update or the validator
/// correctly refuses a directory entry pointing at no managed service.
///
/// Dropping an entry is a directory change, so it advances the publication
/// counter. It did not, and a consumer holding the entry that was just
/// removed saw a generation telling it its copy was current.
fn remove_directory_declaration(document: &mut Value, name: &str) {
    let Some(services) = document
        .get_mut("service_directory")
        .and_then(Value::as_object_mut)
        .and_then(|directory| directory.get_mut("services"))
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    if services.remove(name).is_none() {
        return;
    }
    // A directory that cannot carry a counter is a document this command did
    // not write and must not silently repair; the removal still stands.
    let _ = crate::service_resolution::advance_generation(document);
}

/// Serialize an operator lifecycle mutation with the autonomy reconciler.
///
/// A reconciler tick takes its service snapshot before it takes the per-unit
/// lease. Holding the same lease across withdrawal and the host action makes a
/// stale tick stop at that boundary instead of starting the unit between the
/// stop body and its postcondition probe.
async fn with_service_mutation_subject<T, F, Fut>(
    host: &str,
    unit: &str,
    operation: F,
) -> Result<T, CmdError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<T, CmdError>>,
{
    let store = beacon_store().await?;
    let subject = format!("service:{host}:{unit}");
    let decision = format!(
        "service-lifecycle-{}",
        chrono::Utc::now().timestamp_micros()
    );
    let mut lease = None;
    for _ in 0..300 {
        lease = crate::autonomy::storage::acquire_placement_lease(
            &store,
            &subject,
            &decision,
            "service-lifecycle",
            1800,
            chrono::Utc::now(),
        )
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
        if lease.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    let lease = lease.ok_or_else(|| {
        CmdError::click(format!(
            "{subject} stayed under another mutation lease for 300 seconds"
        ))
    })?;
    let result = operation().await;
    let released =
        crate::autonomy::storage::release_placement_lease(&store, &subject, &lease.token).await;
    match (result, released) {
        (Ok(value), Ok(true)) => Ok(value),
        (Ok(_), Ok(false)) => Err(CmdError::click(format!(
            "{subject} changed lease ownership before the lifecycle operation completed"
        ))),
        (Ok(_), Err(error)) => Err(CmdError::click(format!(
            "{subject} completed, but releasing its mutation lease failed: {error}"
        ))),
        (Err(error), Ok(true)) => Err(error),
        (Err(error), Ok(false)) => Err(CmdError::click(format!(
            "{error}; {subject} changed lease ownership before failure cleanup completed"
        ))),
        (Err(error), Err(release)) => Err(CmdError::click(format!(
            "{error}; releasing the {subject} mutation lease also failed: {release}"
        ))),
    }
}

async fn with_service_mutation_lease<T, F, Fut>(
    service: &ManagedService,
    operation: F,
) -> Result<T, CmdError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<T, CmdError>>,
{
    with_service_mutation_subject(&service.host, service.unit_id(), operation).await
}

#[derive(Clone)]
struct ReconcilerFence {
    baseline_report: Option<String>,
    timeout_seconds: u64,
}

fn active_coordinator_interval(document: &Value) -> Option<u64> {
    document
        .get("coordinators")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|coordinator| coordinator.get("active").and_then(Value::as_bool) == Some(true))
        .filter_map(|coordinator| coordinator.get("interval_seconds").and_then(Value::as_u64))
        .max()
        .map(|seconds| seconds.clamp(15, 600))
}

async fn reconciler_report_id(store: &JobStorage) -> Result<Option<String>, CmdError> {
    crate::autonomy::storage::read_json::<
        crate::autonomy::service_reconciler::ServiceReconcileReport,
    >(
        store,
        crate::autonomy::service_reconciler::LATEST_REPORT,
    )
    .await
    .map(|report| report.map(|report| report.created_at))
    .map_err(|error| CmdError::click(error.to_string()))
}

async fn capture_reconciler_fence(document: &Value) -> Result<Option<ReconcilerFence>, CmdError> {
    let Some(interval) = active_coordinator_interval(document) else {
        return Ok(None);
    };
    let store = beacon_store().await?;
    Ok(Some(ReconcilerFence {
        baseline_report: reconciler_report_id(&store).await?,
        timeout_seconds: interval.saturating_mul(2).saturating_add(60).min(900),
    }))
}

async fn wait_for_reconciler_fence(fence: Option<&ReconcilerFence>) -> Result<(), CmdError> {
    let Some(fence) = fence else {
        return Ok(());
    };
    let store = beacon_store().await?;
    let started = tokio::time::Instant::now();
    let timeout = std::time::Duration::from_secs(fence.timeout_seconds);
    loop {
        let read_error = match reconciler_report_id(&store).await {
            Ok(Some(current)) if Some(&current) != fence.baseline_report.as_ref() => return Ok(()),
            Ok(_) => None,
            Err(error) => Some(error.to_string()),
        };
        if started.elapsed() >= timeout {
            let detail = read_error
                .map(|error| format!("; the last report read failed: {error}"))
                .unwrap_or_default();
            return Err(CmdError::click(format!(
                "the active coordinator published no newer service-reconcile report within {} seconds{detail}",
                fence.timeout_seconds
            )));
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

/// Remove a declaration before stopping its unit.
///
/// A coordinator from before the per-service lease can already hold an old
/// snapshot. The report fence waits until that pass has published its result;
/// after that publication no action from the old snapshot remains in flight,
/// while every later pass sees the withdrawn declaration.
async fn suspend_service_declaration(
    host: &str,
    unit: &str,
) -> Result<(ManagedService, String, Option<ReconcilerFence>), CmdError> {
    let (mut document, expected_generation) = registry::fetch_versioned_document().await?;
    let fence = capture_reconciler_fence(&document).await?;
    let removed = service::remove_service(&mut document, host, unit).map_err(click)?;
    let generation = registry::push_document_if(&document, &expected_generation).await?;
    Ok((removed, generation, fence))
}

async fn restore_service_declaration(service: &ManagedService) -> Result<String, CmdError> {
    let record = service.to_record();
    registry::commit_document(|document| {
        let already_restored = document
            .get("targets")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|target| target.get("name").and_then(Value::as_str) == Some(&service.host))
            .and_then(|target| target.get("services"))
            .and_then(Value::as_array)
            .is_some_and(|services| services.iter().any(|candidate| candidate == &record));
        if already_restored {
            return Ok(document.clone());
        }
        let mut restored = document.clone();
        service::add_service(&mut restored, service).map_err(click)?;
        Ok(restored)
    })
    .await
}

async fn retirement_failure(service: &ManagedService, failure: String) -> CmdError {
    match restore_service_declaration(service).await {
        Ok(generation) => CmdError::click(format!(
            "{failure}; the registry declaration was restored at generation {generation}"
        )),
        Err(restore) => CmdError::click(format!(
            "{failure}; restoring the registry declaration also failed: {restore}"
        )),
    }
}

async fn finish_directory_retirement(
    service: &ManagedService,
    query: &str,
) -> Result<String, CmdError> {
    registry::commit_document(|document| {
        let mut next = document.clone();
        remove_directory_declaration(&mut next, &service.name);
        if service.unit_id() != service.name {
            remove_directory_declaration(&mut next, service.unit_id());
        }
        if query != service.name && query != service.unit_id() {
            remove_directory_declaration(&mut next, query);
        }
        Ok(next)
    })
    .await
}

/// Repeat only a host-confirmed retirement whose immediate end-state probe
/// caught one last external start. The declaration stays withdrawn for the
/// whole loop, and every pass reapplies the init-system fence; transport
/// failures and explicit host refusals are never retried.
async fn retire_service_stably(
    target: &crate::targets::ComputeTarget,
    service: &ManagedService,
    sudo_password: Option<&str>,
    runner: &crate::deploy::Runner,
) -> Result<service::RemoteReport, DeployError> {
    let mut report = service::retire_service(target, service, sudo_password, runner).await?;
    for _ in 1..6 {
        if report.succeeded("retired")
            || report.status != "retired"
            || report.postcondition_state != host_channel::POSTCONDITION_UNMET
        {
            return Ok(report);
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        report = service::retire_service(target, service, sudo_password, runner).await?;
    }
    Ok(report)
}
async fn remote_file_identity(
    target: &crate::targets::ComputeTarget,
    path: &str,
    runner: &crate::deploy::Runner,
) -> Result<Value, CmdError> {
    for words in [
        vec!["/bin/test", "-f", path],
        vec!["/bin/test", "!", "-L", path],
    ] {
        let shape = host_channel::run_program(target, &words, runner)
            .await
            .map_err(click)?;
        if !shape.ok() {
            return Err(CmdError::click(format!(
                "{}: obsolete release-control artifact {path} must be a regular non-symlink file",
                target.name
            )));
        }
    }
    let digest = host_channel::run_program(target, &["/usr/bin/shasum", "-a", "256", path], runner)
        .await
        .map_err(click)?;
    if !digest.ok() {
        return Err(CmdError::click(format!(
            "{}: cannot hash obsolete release-control artifact {path}: {}",
            target.name,
            host_channel::last_error_line(&digest, "shasum failed")
        )));
    }
    let sha256 = digest
        .stdout
        .split_whitespace()
        .next()
        .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or_else(|| {
            CmdError::click(format!(
                "{}: shasum returned no SHA-256 for {path}",
                target.name
            ))
        })?;
    let metadata =
        host_channel::run_program(target, &["/usr/bin/stat", "-f", "%z %Lp", path], runner)
            .await
            .map_err(click)?;
    if !metadata.ok() {
        return Err(CmdError::click(format!(
            "{}: cannot inspect obsolete release-control artifact {path}: {}",
            target.name,
            host_channel::last_error_line(&metadata, "stat failed")
        )));
    }
    let mut fields = metadata.stdout.split_ascii_whitespace();
    let size = fields
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| {
            CmdError::click(format!(
                "{}: stat returned no byte size for {path}",
                target.name
            ))
        })?;
    let mode = fields
        .next()
        .and_then(|value| u32::from_str_radix(value, 8).ok())
        .filter(|mode| *mode <= 0o7777)
        .map(|mode| format!("{mode:04o}"))
        .ok_or_else(|| {
            CmdError::click(format!(
                "{}: stat returned no four-digit mode for {path}",
                target.name
            ))
        })?;
    let transaction = format!(
        "{}-{}",
        chrono::Utc::now().format("%Y%m%dT%H%M%SZ"),
        uuid::Uuid::new_v4().simple()
    );
    Ok(json!({
        "path": path,
        "transaction": transaction,
        "sha256": sha256,
        "size": size,
        "mode": mode,
    }))
}
async fn require_no_executable_caller(
    target: &crate::targets::ComputeTarget,
    path: &str,
    runner: &crate::deploy::Runner,
) -> Result<(), CmdError> {
    let processes =
        host_channel::run_program(target, &["/bin/ps", "-axo", "pid=,command="], runner)
            .await
            .map_err(click)?;
    if !processes.ok() {
        return Err(CmdError::click(format!(
            "{}: cannot prove obsolete executable {path} has no caller: {}",
            target.name,
            host_channel::last_error_line(&processes, "ps failed")
        )));
    }
    if let Some(caller) = processes.stdout.lines().find(|line| {
        line.split_ascii_whitespace()
            .skip(1)
            .any(|argument| argument == path)
    }) {
        return Err(CmdError::click(format!(
            "{}: obsolete executable {path} is still referenced by process {}",
            target.name,
            caller.trim()
        )));
    }
    Ok(())
}

fn externalize_release_controlled_profile(
    document: &mut Value,
    profile: &str,
    service_name: &str,
    product: &str,
) -> Result<(), CmdError> {
    let profiles = document
        .get_mut("placement_profiles")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| CmdError::click("registry.placement_profiles is not an array"))?;
    let profile = profiles
        .iter_mut()
        .find(|entry| entry.get("name").and_then(Value::as_str) == Some(profile))
        .ok_or_else(|| CmdError::click(format!("placement profile {profile:?} disappeared")))?;
    let hosts = profile
        .get_mut("hosts")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| CmdError::click("placement profile hosts is not an object"))?;
    for (host, template) in hosts {
        let units = template
            .get_mut("units")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                CmdError::click(format!("placement host {host:?} units is not an object"))
            })?;
        if !units.contains_key(service_name) {
            return Err(CmdError::click(format!(
                "placement host {host:?} has no template for service {service_name:?}"
            )));
        }
        units.insert(
            service_name.to_string(),
            json!({
                "name": service_name,
                "controller": "release-control",
                "product": product,
            }),
        );
    }
    Ok(())
}

fn remove_release_legacy_identity(
    document: &mut Value,
    product: &str,
    host: &str,
) -> Result<(), CmdError> {
    let target = document
        .get_mut("release_control")
        .and_then(|control| control.get_mut("products"))
        .and_then(|products| products.get_mut(product))
        .and_then(|policy| policy.get_mut("targets"))
        .and_then(|targets| targets.get_mut(host))
        .and_then(Value::as_object_mut)
        .ok_or_else(|| {
            CmdError::click(format!(
                "release-control product {product:?} target {host:?} disappeared"
            ))
        })?;
    target.remove("legacy_launchd_label");
    target.remove("legacy_launchd_plist");
    let generation = document
        .get_mut("release_control")
        .and_then(Value::as_object_mut)
        .and_then(|control| control.get_mut("generation"))
        .and_then(|value| value.as_u64())
        .ok_or_else(|| CmdError::click("registry.release_control.generation is not an integer"))?;
    document["release_control"]["generation"] = Value::from(generation.saturating_add(1));
    Ok(())
}

fn document_contains_string(value: &Value, needle: &str) -> bool {
    match value {
        Value::String(value) => value == needle,
        Value::Array(values) => values
            .iter()
            .any(|value| document_contains_string(value, needle)),
        Value::Object(values) => values
            .values()
            .any(|value| document_contains_string(value, needle)),
        _ => false,
    }
}
fn handoff_receipt_path(product: &str, version: &str, host: &str) -> std::path::PathBuf {
    crate::config_file::expand_tilde("~")
        .join(".stado/work/service-release")
        .join(product)
        .join(version)
        .join(format!("handoff-{host}.json"))
}

fn persist_handoff_receipt(
    path: &std::path::Path,
    report: &Value,
    replace: bool,
) -> Result<(), CmdError> {
    use std::io::Write as _;

    let parent = path
        .parent()
        .ok_or_else(|| CmdError::click("handoff receipt path has no parent"))?;
    std::fs::create_dir_all(parent)?;
    let mut bytes = serde_json::to_vec_pretty(report)?;
    bytes.push(b'\n');
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    staged.write_all(&bytes)?;
    staged.as_file().sync_all()?;
    if replace {
        staged.persist(path).map_err(|error| error.error)?;
    } else if let Err(error) = staged.persist_noclobber(path) {
        let existing = std::fs::read(path)?;
        if existing != bytes {
            return Err(CmdError::click(format!(
                "handoff receipt {} already exists with different content",
                path.display()
            )));
        }
        drop(error);
    }
    std::fs::File::open(parent)?.sync_all()?;
    Ok(())
}

fn read_handoff_receipt(path: &std::path::Path) -> Result<Option<Value>, CmdError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes).map_err(|error| {
            CmdError::click(format!(
                "handoff receipt {} is invalid JSON: {error}",
                path.display()
            ))
        })?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn registry_has_intended_handoff(
    document: &Value,
    profile: &str,
    service_name: &str,
    product: &str,
    host: &str,
    legacy_identities: [&str; 3],
) -> bool {
    let units_external = document
        .get("placement_profiles")
        .and_then(Value::as_array)
        .and_then(|profiles| {
            profiles
                .iter()
                .find(|entry| entry.get("name").and_then(Value::as_str) == Some(profile))
        })
        .and_then(|profile| profile.get("hosts"))
        .and_then(Value::as_object)
        .map(|hosts| {
            !hosts.is_empty()
                && hosts.values().all(|template| {
                    template
                        .get("units")
                        .and_then(|units| units.get(service_name))
                        .is_some_and(|unit| {
                            unit.get("controller").and_then(Value::as_str)
                                == Some("release-control")
                                && unit.get("product").and_then(Value::as_str) == Some(product)
                        })
                })
        })
        == Some(true);
    let legacy_removed = document
        .get("release_control")
        .and_then(|control| control.get("products"))
        .and_then(|products| products.get(product))
        .and_then(|policy| policy.get("targets"))
        .and_then(|targets| targets.get(host))
        .is_some_and(|target| {
            target.get("legacy_launchd_label").is_none()
                && target.get("legacy_launchd_plist").is_none()
        });
    let legacy_unreachable = legacy_identities
        .into_iter()
        .all(|identity| !identity.is_empty() && !document_contains_string(document, identity));
    units_external && legacy_removed && legacy_unreachable
}

fn same_remote_file_identity(left: &Value, right: &Value) -> bool {
    ["path", "sha256", "size", "mode"]
        .into_iter()
        .all(|field| left.get(field) == right.get(field))
}

async fn finish_handoff_under_lease(
    document: &Value,
    target: &crate::targets::ComputeTarget,
    installed_stado: &str,
    receipt_path: &std::path::Path,
    mut report: Value,
    observed_generation: Option<&str>,
    json_output: bool,
) -> Result<(), CmdError> {
    let host = report["host"]
        .as_str()
        .ok_or_else(|| CmdError::click("handoff receipt has no host"))?
        .to_owned();
    let product = report["product"]
        .as_str()
        .ok_or_else(|| CmdError::click("handoff receipt has no product"))?
        .to_owned();
    let service_name = report["service"]
        .as_str()
        .ok_or_else(|| CmdError::click("handoff receipt has no service"))?
        .to_owned();
    let legacy_label = report["legacy"]["label"]
        .as_str()
        .ok_or_else(|| CmdError::click("handoff receipt has no legacy label"))?
        .to_owned();
    let legacy_program = report["retirement"]["binary_receipt"]["path"]
        .as_str()
        .ok_or_else(|| CmdError::click("handoff receipt has no legacy binary path"))?
        .to_owned();
    let runner = production_runner();

    if report["generation"].is_null() {
        let observed_generation = observed_generation.ok_or_else(|| {
            CmdError::click("recovered handoff has no observed registry generation")
        })?;
        report["recovery"] = json!({
            "original_cas_generation": Value::Null,
            "original_cas_generation_status": "unknown_after_interruption",
            "observed_registry_generation": observed_generation,
        });
    }
    report["status"] = json!("registry_committed");
    persist_handoff_receipt(receipt_path, &report, true)?;
    let fence_capture = capture_reconciler_fence(document).await;
    report["reconciler_fence"] = json!({
        "status": if fence_capture.is_ok() { "pending" } else { "capture_failed" },
        "baseline_report_id": fence_capture
            .as_ref()
            .ok()
            .and_then(Option::as_ref)
            .and_then(|fence| fence.baseline_report.as_deref()),
        "timeout_seconds": fence_capture
            .as_ref()
            .ok()
            .and_then(Option::as_ref)
            .map(|fence| fence.timeout_seconds),
        "error": fence_capture.as_ref().err().map(ToString::to_string),
    });
    persist_handoff_receipt(receipt_path, &report, true)?;
    let active = host_channel::run_program(
        target,
        &[
            installed_stado,
            "release",
            "active-binary",
            &product,
            "--target",
            &host,
            "--json",
        ],
        &runner,
    )
    .await
    .map_err(click)?;
    if !active.ok() {
        return Err(CmdError::click(format!(
            "{host}: installed Stado rejected active release binary: {}",
            host_channel::last_error_line(&active, "active-binary failed")
        )));
    }
    let active: Value = serde_json::from_str(active.stdout.trim()).map_err(|error| {
        CmdError::click(format!(
            "{host}: active-binary returned invalid JSON: {error}"
        ))
    })?;
    if active["state"] != "active" || active["product"] != product || active["target"] != host {
        return Err(CmdError::click(format!(
            "{host}: active-binary identity no longer matches the durable handoff receipt"
        )));
    }
    for field in ["version", "artifact_sha256", "manifest_sha256"] {
        if active[field] != report["release"][field] {
            return Err(CmdError::click(format!(
                "{host}: active release {field} no longer matches the durable handoff receipt"
            )));
        }
    }
    let fence = fence_capture?;
    wait_for_reconciler_fence(fence.as_ref()).await?;
    let label = service_label_print::print_label(
        target,
        &legacy_label,
        service::BootoutScope::System,
        &runner,
    )
    .await
    .map_err(click)?;
    if label.loaded() {
        return Err(CmdError::click(format!(
            "{host}: legacy launchd label {legacy_label:?} was restarted after registry handoff"
        )));
    }
    require_no_executable_caller(target, &legacy_program, &runner).await?;
    report["status"] = json!("handed_off");
    report["retirement"]["status"] = json!("eligible");
    report["reconciler_fence"]["status"] = json!("satisfied");
    persist_handoff_receipt(receipt_path, &report, true)?;
    if json_output {
        print_json(&report)
    } else {
        println!(
            "{host}: {service_name} handed to release-control product {product} at registry generation {}",
            report["generation"]
        );
        Ok(())
    }
}

async fn finish_committed_handoff(
    document: &Value,
    target: &crate::targets::ComputeTarget,
    installed_stado: &str,
    receipt_path: &std::path::Path,
    report: Value,
    observed_generation: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    let host = report["host"]
        .as_str()
        .ok_or_else(|| CmdError::click("handoff receipt has no host"))?
        .to_owned();
    let legacy_label = report["legacy"]["label"]
        .as_str()
        .ok_or_else(|| CmdError::click("handoff receipt has no legacy label"))?
        .to_owned();
    with_service_mutation_subject(&host, &legacy_label, || {
        finish_handoff_under_lease(
            document,
            target,
            installed_stado,
            receipt_path,
            report,
            Some(observed_generation),
            json_output,
        )
    })
    .await
}
/// Transfer one placed service from generic unit lifecycle to release-control.
///
/// Every runtime fact is established before the sole registry CAS. The target
/// service row, placement restart data, and release fallback identity disappear
/// together, so no intermediate registry can restart the legacy executable.
async fn handoff_release_control(
    service_name: &str,
    host: &str,
    product: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    let (mut document, expected_generation) = registry::fetch_versioned_document().await?;
    crate::targets::validate_registry(&document)
        .map_err(|error| CmdError::click(error.to_string()))?;
    let registry_model = targets::load_registry_from_str(&serde_json::to_string(&document)?)
        .map_err(|error| CmdError::click(error.to_string()))?;
    let target = host_channel::resolve_target(&registry_model, host)
        .map_err(click)?
        .clone();
    let directory = crate::service_resolution::directory(&document)
        .map_err(CmdError::click)?
        .ok_or_else(|| CmdError::click("registry.service_directory is not configured"))?;
    let route = directory.services.get(service_name).ok_or_else(|| {
        CmdError::click(format!(
            "service directory declares no service {service_name:?}"
        ))
    })?;
    if route.active_host != host {
        return Err(CmdError::click(format!(
            "service {service_name:?} is active on {:?}, not {host:?}",
            route.active_host
        )));
    }
    let profile_name = route.placement_profile.as_deref().ok_or_else(|| {
        CmdError::click(format!(
            "service {service_name:?} is not backed by a placement profile"
        ))
    })?;
    let profile = crate::placement::profiles(&document)
        .map_err(CmdError::click)?
        .into_iter()
        .find(|profile| profile.name == profile_name)
        .ok_or_else(|| CmdError::click(format!("placement profile {profile_name:?} is absent")))?;
    if let Some(transaction) = crate::placement::transactions(&document)
        .map_err(CmdError::click)?
        .into_iter()
        .find(|transaction| transaction.profile == profile_name)
    {
        return Err(CmdError::click(format!(
            "placement profile {profile_name:?} is owned by active transaction {:?}",
            transaction.id
        )));
    }

    let control = crate::release_control::control(&document)?
        .ok_or_else(|| CmdError::click("registry.release_control is not configured"))?;
    let policy = control.products.get(product).ok_or_else(|| {
        CmdError::click(format!(
            "release-control product {product:?} is not declared"
        ))
    })?;
    if policy.service != service_name {
        return Err(CmdError::click(format!(
            "release-control product {product:?} owns service {:?}, not {service_name:?}",
            policy.service
        )));
    }
    let target_policy = policy.targets.get(host).ok_or_else(|| {
        CmdError::click(format!(
            "release-control product {product:?} has no target {host:?}"
        ))
    })?;
    let desired = policy.desired.as_ref().ok_or_else(|| {
        CmdError::click(format!(
            "release-control product {product:?} has no desired release"
        ))
    })?;
    let desired_artifact = desired
        .artifacts
        .get(&target_policy.platform)
        .ok_or_else(|| {
            CmdError::click(format!(
                "desired release {:?} has no artifact for {}",
                desired.version, target_policy.platform
            ))
        })?;
    let receipt_path = handoff_receipt_path(product, &desired.version, host);
    let prior_receipt = read_handoff_receipt(&receipt_path)?;
    if let Some(receipt) = prior_receipt.as_ref() {
        let same_intent = receipt["schema"] == "stado.service-release-control-handoff.v1"
            && receipt["service"] == service_name
            && receipt["host"] == host
            && receipt["profile"] == profile_name
            && receipt["product"] == product
            && receipt["release"]["version"] == desired.version
            && receipt["release"]["artifact_sha256"] == desired_artifact.artifact_sha256
            && receipt["release"]["manifest_sha256"] == desired_artifact.manifest_sha256;
        if !same_intent {
            return Err(CmdError::click(format!(
                "handoff receipt {} records a different operation",
                receipt_path.display()
            )));
        }
        let receipt_label = receipt["legacy"]["label"]
            .as_str()
            .ok_or_else(|| CmdError::click("handoff receipt has no legacy label"))?;
        let receipt_plist = receipt["retirement"]["plist_receipt"]["path"]
            .as_str()
            .ok_or_else(|| CmdError::click("handoff receipt has no legacy plist path"))?;
        let receipt_program = receipt["retirement"]["binary_receipt"]["path"]
            .as_str()
            .ok_or_else(|| CmdError::click("handoff receipt has no legacy binary path"))?;
        if registry_has_intended_handoff(
            &document,
            profile_name,
            service_name,
            product,
            host,
            [receipt_label, receipt_plist, receipt_program],
        ) {
            let installed_stado = format!("{}/.stado/bin/stado", target_policy.home);
            return finish_committed_handoff(
                &document,
                &target,
                &installed_stado,
                &receipt_path,
                receipt.clone(),
                &expected_generation,
                json_output,
            )
            .await;
        }
        if receipt["status"] != "prepared" {
            return Err(CmdError::click(format!(
                "handoff receipt {} says {:?}, but the registry does not match its intended handoff",
                receipt_path.display(),
                receipt["status"]
            )));
        }
    }
    for (template_host, template) in &profile.hosts {
        let unit = template.units.get(service_name).ok_or_else(|| {
            CmdError::click(format!(
                "placement host {template_host:?} has no {service_name:?} template"
            ))
        })?;
        if unit.managed().is_none() {
            return Err(CmdError::click(format!(
                "placement service {service_name:?} is already release-controlled"
            )));
        }
    }
    let legacy_label = target_policy
        .legacy_launchd_label
        .as_deref()
        .ok_or_else(|| {
            CmdError::click(format!(
                "release-control target {host:?} has no legacy launchd label to hand off"
            ))
        })?;
    let legacy_plist = target_policy
        .legacy_launchd_plist
        .as_deref()
        .ok_or_else(|| {
            CmdError::click(format!(
                "release-control target {host:?} has no legacy launchd plist to hand off"
            ))
        })?;
    let legacy = service::declared_services(&target)
        .into_iter()
        .find(|declared| declared.name == service_name || declared.unit_id() == legacy_label)
        .ok_or_else(|| {
            CmdError::click(format!(
                "{service_name:?} has no active target-managed legacy service row on {host:?}"
            ))
        })?;
    if legacy.unit_id() != legacy_label || legacy.path != legacy_plist {
        return Err(CmdError::click(format!(
            "legacy service row does not match release-control identity {legacy_label} at \
             {legacy_plist}"
        )));
    }

    with_service_mutation_lease(&legacy, || async {
        let runner = production_runner();
        let state_path = crate::release_agent::host_state_path(&target_policy.state_dir, product);
        let state_text = host_channel::remote_read_file(&target, &state_path, &runner)
            .await
            .map_err(click)?
            .ok_or_else(|| {
                CmdError::click(format!("{host}: release state is absent at {state_path}"))
            })?;
        let state = crate::release_agent::parse_state_document(
            state_text.as_bytes(),
            product,
            host,
            &state_path,
        )
        .map_err(CmdError::click)?;
        let active = state.active.as_ref().ok_or_else(|| {
            CmdError::click(format!(
                "{host}: release-control has no active {product:?} process"
            ))
        })?;
        if state.phase != crate::release_agent::RolloutPhase::Committed
            || state.candidate.is_some()
            || state.proxy_pid.is_none()
            || state.rollout_generation != desired.rollout_generation
            || active.version != desired.version
            || active.artifact_sha256 != desired_artifact.artifact_sha256
            || active.manifest_sha256 != desired_artifact.manifest_sha256
        {
            return Err(CmdError::click(format!(
                "{host}: release-control {product:?} is not the settled desired release"
            )));
        }

        let installed_stado = format!("{}/.stado/bin/stado", target_policy.home);
        let active_binary = host_channel::run_program(
            &target,
            &[
                &installed_stado,
                "release",
                "active-binary",
                product,
                "--target",
                host,
                "--json",
            ],
            &runner,
        )
        .await
        .map_err(click)?;
        if !active_binary.ok() {
            return Err(CmdError::click(format!(
                "{host}: installed Stado rejected active release binary: {}",
                host_channel::last_error_line(&active_binary, "active-binary failed")
            )));
        }
        let active_binary: Value =
            serde_json::from_str(active_binary.stdout.trim()).map_err(|error| {
                CmdError::click(format!(
                    "{host}: active-binary returned invalid JSON: {error}"
                ))
            })?;
        if active_binary["state"] != "active"
            || active_binary["product"] != product
            || active_binary["target"] != host
            || active_binary["version"] != desired.version
            || active_binary["artifact_sha256"] != desired_artifact.artifact_sha256
            || active_binary["manifest_sha256"] != desired_artifact.manifest_sha256
        {
            return Err(CmdError::click(format!(
                "{host}: active-binary identity does not match the desired release"
            )));
        }

        let serving = target_policy
            .blue_green_serving()
            .map_err(CmdError::click)?;
        let readiness_url = format!("http://{}{}", serving.stable_bind, serving.readiness_path);
        let readiness = host_channel::run_program(
            &target,
            &[
                "/usr/bin/curl",
                "--silent",
                "--show-error",
                "--fail",
                "--max-time",
                "3",
                "--output",
                "/dev/null",
                &readiness_url,
            ],
            &runner,
        )
        .await
        .map_err(click)?;
        if !readiness.ok() {
            return Err(CmdError::click(format!(
                "{host}: release-control readiness failed at {readiness_url}: {}",
                host_channel::last_error_line(&readiness, "readiness request failed")
            )));
        }
        let label = service_label_print::print_label(
            &target,
            legacy_label,
            service::BootoutScope::System,
            &runner,
        )
        .await
        .map_err(click)?;
        if label.loaded() {
            return Err(CmdError::click(format!(
                "{host}: legacy launchd label {legacy_label:?} is still loaded"
            )));
        }
        require_no_executable_caller(&target, &legacy.program, &runner).await?;
        let observed_plist_identity = remote_file_identity(&target, legacy_plist, &runner).await?;
        let observed_binary_identity =
            remote_file_identity(&target, &legacy.program, &runner).await?;
        let (plist_identity, binary_identity) = if let Some(receipt) = prior_receipt.as_ref() {
            let stored_plist = &receipt["retirement"]["plist_receipt"];
            let stored_binary = &receipt["retirement"]["binary_receipt"];
            if !same_remote_file_identity(stored_plist, &observed_plist_identity)
                || !same_remote_file_identity(stored_binary, &observed_binary_identity)
            {
                return Err(CmdError::click(format!(
                    "handoff receipt {} no longer matches the exact legacy files",
                    receipt_path.display()
                )));
            }
            (stored_plist.clone(), stored_binary.clone())
        } else {
            (observed_plist_identity, observed_binary_identity)
        };

        service::remove_service(&mut document, host, legacy_label).map_err(click)?;
        externalize_release_controlled_profile(&mut document, profile_name, service_name, product)?;
        remove_release_legacy_identity(&mut document, product, host)?;
        crate::service_resolution::advance_generation(&mut document).map_err(CmdError::click)?;
        for obsolete in [legacy_label, legacy_plist, legacy.program.as_str()] {
            if obsolete.is_empty() || document_contains_string(&document, obsolete) {
                return Err(CmdError::click(format!(
                    "registry handoff left obsolete executable identity {obsolete:?} reachable"
                )));
            }
        }
        crate::targets::validate_registry(&document)
            .map_err(|error| CmdError::click(error.to_string()))?;

        let refreshing_prepared_receipt = prior_receipt.is_some();
        let mut report = if let Some(receipt) = prior_receipt.as_ref() {
            let mut receipt = receipt.clone();
            receipt["expected_generation"] = json!(expected_generation);
            receipt["generation"] = Value::Null;
            receipt["status"] = json!("prepared");
            receipt["reconciler_fence"] = json!({"status": "not_captured"});
            receipt
        } else {
            json!({
                "schema": "stado.service-release-control-handoff.v1",
                "status": "prepared",
                "service": service_name,
                "host": host,
                "profile": profile_name,
                "controller": "release-control",
                "product": product,
                "expected_generation": expected_generation,
                "generation": Value::Null,
                "receipt_path": receipt_path,
                "intended_post_handoff": {
                    "controller": "release-control",
                    "product": product,
                    "legacy_registry_identities_removed": true,
                },
                "release": {
                    "version": active.version,
                    "rollout_generation": state.rollout_generation,
                    "artifact_sha256": active.artifact_sha256,
                    "manifest_sha256": active.manifest_sha256,
                    "proxy_pid": state.proxy_pid,
                    "active_binary": active_binary["path"],
                    "readiness_url": readiness_url,
                },
                "reconciler_fence": {
                    "status": "not_captured",
                },
                "legacy": {
                    "label": legacy_label,
                    "loaded": false,
                    "registry_referrers": [],
                },
                "retirement": {
                    "status": "pending",
                    "order": ["plist", "binary"],
                    "plist_receipt": plist_identity,
                    "binary_receipt": binary_identity,
                },
            })
        };
        persist_handoff_receipt(&receipt_path, &report, refreshing_prepared_receipt)?;
        let generation = registry::push_document_if(&document, &expected_generation).await?;
        report["status"] = json!("registry_committed");
        report["generation"] = json!(generation);
        persist_handoff_receipt(&receipt_path, &report, true)?;

        finish_handoff_under_lease(
            &document,
            &target,
            &installed_stado,
            &receipt_path,
            report,
            None,
            json_output,
        )
        .await
    })
    .await
}

async fn retire(unit: &str, host: &str, json: bool) -> Result<(), CmdError> {
    let target = host_channel::canonical_target(host).await.map_err(click)?;
    let declared = service::declared_services(&target);
    let Some(found) = declared
        .iter()
        .find(|candidate| candidate.matches(unit))
        .cloned()
    else {
        return Err(unmanaged(unit, Some(host)));
    };
    if found.source == SOURCE_RECOVERY {
        return Err(CmdError::click(format!(
            "{unit} is carried by the fixed host-recovery program, not by the registry entry \
             for {host}; it cannot be retired. Adopt it first if you need it under registry \
             management."
        )));
    }
    let runner = production_runner();
    let sudo_password = if UnitDomain::from_path(&found.path).requires_privileged_bootstrap() {
        host_sudo_password(&target).await?
    } else {
        None
    };
    with_service_mutation_lease(&found, || async {
        let (removed, _, fence) = suspend_service_declaration(host, unit).await?;

        if removed.unit_id().is_empty() && removed.path.is_empty() {
            let generation = finish_directory_retirement(&removed, unit).await?;
            return render_mutation("retired", &removed, &generation, None, json);
        }
        if let Err(error) = wait_for_reconciler_fence(fence.as_ref()).await {
            let failure =
                format!("{host}: could not fence {unit} from the active coordinator: {error}");
            return Err(retirement_failure(&removed, failure).await);
        }

        let report =
            match retire_service_stably(&target, &removed, sudo_password.as_deref(), &runner).await
            {
                Ok(report) => report,
                Err(error) => {
                    let failure = format!("{host}: could not stop {unit}: {error}");
                    return Err(retirement_failure(&removed, failure).await);
                }
            };
        if !report.succeeded("retired") {
            let failure = format!("{host}: could not stop {unit}: {}", report.failure());
            return Err(retirement_failure(&removed, failure).await);
        }

        let generation = finish_directory_retirement(&removed, unit).await?;
        render_mutation(
            "retired",
            &removed,
            &generation,
            Some(&report.to_json()),
            json,
        )
    })
    .await
}

/// `service remove`: withdraw the declaration while holding the same mutation
/// lease as the autonomy reconciler, stop the unit, then remove its directory
/// entry and declared unit file. The file path comes from the registry rather
/// than operator input.
///
/// Partial states are said, not hidden: a stopped-and-forgotten service whose
/// file the channel may not delete is `retired` with the file named, and the
/// command exits non-zero because the asked-for end state did not happen.
async fn remove(unit: &str, host: &str, json: bool) -> Result<(), CmdError> {
    let target = host_channel::canonical_target(host).await.map_err(click)?;
    let declared = service::declared_services(&target);
    let Some(found) = declared
        .iter()
        .find(|candidate| candidate.matches(unit))
        .cloned()
    else {
        return Err(unmanaged(unit, Some(host)));
    };
    if found.source == SOURCE_RECOVERY {
        return Err(CmdError::click(format!(
            "{unit} is carried by the fixed host-recovery program, not by the registry entry \
             for {host}; it cannot be removed. Adopt it first if you need it under registry \
             management."
        )));
    }
    let path = found.path.clone();
    let runner = production_runner();
    let sudo_password = if UnitDomain::from_path(&path).requires_privileged_bootstrap() {
        host_sudo_password(&target).await?
    } else {
        None
    };
    with_service_mutation_lease(&found, || async {
        let (removed, _, fence) = suspend_service_declaration(host, unit).await?;

        if removed.unit_id().is_empty() && path.is_empty() {
            let generation = finish_directory_retirement(&removed, unit).await?;
            if json {
                return print_json(&json!({
                    "target": target.name,
                    "unit": unit,
                    "action": "removed",
                    "generation": generation,
                    "file": {"path": "", "status": "absent", "detail": Value::Null},
                    "report": Value::Null,
                }));
            }
            return render_mutation("removed", &removed, &generation, None, false);
        }
        if let Err(error) = wait_for_reconciler_fence(fence.as_ref()).await {
            let failure = format!(
                "{host}: could not fence {unit} from the active coordinator: {error}; its file was not touched"
            );
            return Err(retirement_failure(&removed, failure).await);
        }

        let report =
            match retire_service_stably(&target, &removed, sudo_password.as_deref(), &runner).await
            {
                Ok(report) => report,
                Err(error) => {
                    let failure =
                        format!("{host}: could not stop {unit}: {error}; its file was not touched");
                    return Err(retirement_failure(&removed, failure).await);
                }
            };
        if !report.succeeded("retired") {
            let failure = format!(
                "{host}: could not stop {unit}: {}; its file was not touched",
                report.failure()
            );
            return Err(retirement_failure(&removed, failure).await);
        }

        let generation = finish_directory_retirement(&removed, unit).await?;

        // The registry is already clean: the file half runs last, because a
        // failed delete must leave a service the fleet can still see, not a file
        // nobody declared. Its report is the second document of the answer.
        let file = crate::cli::host::remove_file_document(&target.name, &path).await;
        if json {
            let (file_status, file_detail) = match &file {
                Ok(outcome) => (outcome.status.clone(), outcome.detail.clone()),
                Err(error) => ("failed".to_string(), Some(error.to_string())),
            };
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "target": target.name,
                    "unit": removed.unit_id(),
                    "action": if file.is_ok() { "removed" } else { "retired" },
                    "generation": generation,
                    "report": report.to_json(),
                    "file": {
                        "path": path,
                        "status": file_status,
                        "detail": file_detail,
                    },
                }))?
            );
        } else {
            render_mutation(
                if file.is_ok() { "removed" } else { "retired" },
                &removed,
                &generation,
                Some(&report.to_json()),
                false,
            )?;
            match &file {
                Ok(outcome) => println!("file {}: {}", outcome.status, outcome.path),
                Err(error) => eprintln!("file failed: {error}"),
            }
        }
        file.map(|_| ())
    })
    .await
}

/// One declared service name: lowercase letters and digits at the edges,
/// with '.', '-' and '_' inside — the same rule the directory validator
/// applies, enforced here so a bad name fails before the document moves.
fn declaration_name_ok(value: &str) -> bool {
    let edge = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && edge(bytes[0])
        && edge(bytes[bytes.len() - 1])
        && bytes
            .iter()
            .all(|byte| edge(*byte) || matches!(byte, b'.' | b'_' | b'-'))
}

/// Write one user-authored declaration into the service directory.
///
/// The declaration file is the whole contract: `name` and `host`, the
/// immutable `source`, the opaque `run` spec, and optionally `verify`,
/// `consumers`, and `endpoints` (or `port` as the loopback shorthand).
/// The fleet learns nothing about the service's kind from it — that is the
/// design, not a gap: anything the service knows about itself lives inside
/// the artifact and the run spec.
async fn declare(file: &str, as_json: bool) -> Result<(), CmdError> {
    let text = std::fs::read_to_string(file)
        .map_err(|error| CmdError::click(format!("{file}: {error}")))?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|error| CmdError::click(format!("{file}: not a JSON object: {error}")))?;
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| CmdError::usage(format!("{file}: 'name' is required")))?;
    if !declaration_name_ok(name) {
        return Err(CmdError::usage(format!(
            "{file}: 'name' must be a lowercase identifier without empty edges"
        )));
    }
    let host = value
        .get("host")
        .and_then(Value::as_str)
        .ok_or_else(|| CmdError::usage(format!("{file}: 'host' is required")))?;
    let mut declaration_value = match value.get("declaration") {
        Some(declaration) => declaration.clone(),
        None => {
            let mut assembled = json!({"source": value.get("source")});
            if let Some(run) = value.get("run") {
                assembled["run"] = run.clone();
            }
            assembled
        }
    };
    let declaration: crate::declaration::ServiceDeclaration =
        serde_json::from_value(std::mem::take(&mut declaration_value)).map_err(|error| {
            CmdError::usage(format!(
                "{file}: declaration must carry source.artifact and source.sha256: {error}"
            ))
        })?;
    let location = format!("service_directory.services.{name}");
    let problems = crate::declaration::validate(&location, &declaration);
    if !problems.is_empty() {
        return Err(CmdError::usage(problems.join("; ")));
    }
    let verify = match value.get("verify") {
        Some(descriptor) => {
            let descriptor: targets::VerifyDescriptor = serde_json::from_value(descriptor.clone())
                .map_err(|error| CmdError::usage(format!("{file}: 'verify': {error}")))?;
            let problems = targets::validate_verification(&location, &descriptor);
            if !problems.is_empty() {
                return Err(CmdError::usage(problems.join("; ")));
            }
            Some(
                serde_json::to_value(&descriptor)
                    .map_err(|error| CmdError::click(format!("verify: {error}")))?,
            )
        }
        None => None,
    };
    // Endpoints: explicit map wins; `port` is the loopback shorthand for the
    // declared host. The directory contract requires at least the active
    // host's endpoint, so neither present is an author error, not a default.
    let mut endpoints = value
        .get("endpoints")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let (Some(port), false) = (
        value.get("port").and_then(Value::as_u64),
        endpoints.contains_key(host),
    ) {
        if port == 0 || port > u16::MAX as u64 {
            return Err(CmdError::usage(format!("{file}: 'port' out of range")));
        }
        endpoints.insert(
            host.to_string(),
            json!({"url": format!("http://127.0.0.1:{port}")}),
        );
    }
    if !endpoints.contains_key(host) {
        return Err(CmdError::usage(format!(
            "{file}: the declaration needs an endpoint for {host} — pass 'endpoints' or the 'port' shorthand"
        )));
    }
    // The directory answers "who may call it" from the same entry, so a
    // declaration without consumers is not a declaration the contract can
    // publish: it would answer that question with silence.
    match value.get("consumers").and_then(Value::as_object) {
        Some(consumers) if !consumers.is_empty() => {}
        _ => {
            return Err(CmdError::usage(format!(
                "{file}: 'consumers' is required and must name at least one caller"
            )));
        }
    }

    // Pure: the whole entry is a function of the file that was just parsed
    // and of the document it lands in, so a lost race re-applies it to the
    // newer document. `advance_generation` runs INSIDE the transform because
    // it derives the next counter from the document it was handed; deriving
    // it once from the first read and reusing it after a retry would publish
    // a number the newer document has already used.
    let generation = registry::commit_document(|current| {
        let mut document = current.clone();
        let known_target = document
            .get("targets")
            .and_then(Value::as_array)
            .is_some_and(|targets| {
                targets
                    .iter()
                    .any(|target| target.get("name").and_then(Value::as_str) == Some(host))
            });
        if !known_target {
            return Err(CmdError::usage(format!(
                "{file}: 'host' names {host}, which is not a registry target"
            )));
        }
        let directory = document
            .get_mut("service_directory")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                CmdError::click(
                    "registry has no service_directory; an authority must publish it before services can be declared",
                )
            })?;
        let services = directory
            .entry("services")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or_else(|| CmdError::click("service_directory.services: must be an object"))?;
        let entry = services
            .entry(name.to_string())
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or_else(|| {
                CmdError::click(format!(
                    "service_directory.services.{name}: must be an object"
                ))
            })?;
        entry.insert("active_host".to_string(), json!(host));
        entry.insert("endpoints".to_string(), Value::Object(endpoints.clone()));
        // The directory contract binds every fixed route to the managed unit on
        // its active host, and the unit lands when `deploy` runs — so declare
        // writes the link now and a placeholder record on the host, which
        // `deploy` replaces with the real one. Declared-but-not-yet-deployed is
        // a designed state, not an error: the registry says what should run,
        // the beacons say what does.
        entry.insert("managed_service".to_string(), json!(name));
        if let Some(consumers) = value.get("consumers") {
            entry.insert("consumers".to_string(), consumers.clone());
        }
        if let Some(descriptor) = verify.clone() {
            entry.insert("verify".to_string(), descriptor);
        }
        entry.insert(
            "declaration".to_string(),
            serde_json::to_value(&declaration)
                .map_err(|error| CmdError::click(format!("declaration: {error}")))?,
        );
        let target_entry = document
            .get_mut("targets")
            .and_then(Value::as_array_mut)
            .and_then(|targets| {
                targets
                    .iter_mut()
                    .find(|target| target.get("name").and_then(Value::as_str) == Some(host))
            })
            .ok_or_else(|| CmdError::click(format!("registry targets lost {host}")))?;
        let host_services = target_entry
            .as_object_mut()
            .ok_or_else(|| CmdError::click("registry target: must be an object"))?
            .entry("services")
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or_else(|| {
                CmdError::click(format!("registry target {host}: services must be an array"))
            })?;
        let already = host_services
            .iter()
            .any(|record| record.get("name").and_then(Value::as_str) == Some(name));
        if !already {
            host_services.push(json!({"name": name, "declared_only": true}));
        }
        // `declare` writes `service_directory.services.<name>` above, so the
        // publication counter must advance with it or a consumer's cached copy
        // never learns the entry exists.
        crate::service_resolution::advance_generation(&mut document).map_err(CmdError::click)?;
        Ok(document)
    })
    .await?;
    if !as_json {
        println!("declared {name} on {host} (generation {generation})");
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "declared": name,
                "host": host,
                "generation": generation,
            }))
            .map_err(|error| CmdError::click(format!("declare report: {error}")))?
        );
    }
    Ok(())
}

struct DeployOptions<'a> {
    name: &'a str,
    host: Option<&'a str>,
    host_heuristic: Option<&'a str>,
    from: Option<String>,
    from_artifact: Option<String>,
    args: &'a [String],
    launchd_label: Option<&'a str>,
    as_launch_agent: bool,
    as_json: bool,
}

async fn deploy(options: DeployOptions<'_>) -> Result<(), CmdError> {
    let DeployOptions {
        name,
        host,
        host_heuristic,
        from,
        from_artifact,
        args,
        launchd_label,
        as_launch_agent,
        as_json,
    } = options;
    let (target, host_heuristic) = resolve_placement(host, host_heuristic).await?;
    if (launchd_label.is_some() || as_launch_agent)
        && !target.release_platform.starts_with("darwin")
    {
        return Err(CmdError::click(
            "--launchd-label and --as-launch-agent are Darwin-only",
        ));
    }
    let host = target.name.clone();
    // Exactly one source. Neither is a sensible default: a path deploys
    // whatever is on the host with no version identity, and an artifact
    // deploys a named version; guessing between them is how a host ends up
    // running something nobody can name.
    let mut declaration_args: Vec<String> = Vec::new();
    let (from, installed) = match (from, from_artifact) {
        (Some(path), None) => (path, None),
        (None, Some(reference)) => {
            let installed = install_from_artifact(&target, name, &reference).await?;
            (installed.program_path.clone(), Some(installed))
        }
        (None, None) => {
            // A declared service deploys from its declaration: `service
            // declare` already wrote the artifact reference and the run
            // spec, so the name alone is enough.
            let document = registry::fetch_document().await?;
            let entry = document
                .get("service_directory")
                .and_then(|directory| directory.get("services"))
                .and_then(|services| services.get(name))
                .cloned();
            let Some(entry) = entry else {
                return Err(CmdError::click(format!(
                    "deploy needs --from PATH or --from-artifact REF, or a declaration written by \
                     `stado service declare --file`; the directory names no service '{name}'"
                )));
            };
            let Some(declared) = crate::declaration::ServiceDeclaration::from_entry(&entry) else {
                return Err(CmdError::click(format!(
                    "deploy needs --from PATH or --from-artifact REF: '{name}' is declared without a source"
                )));
            };
            let installed = install_from_artifact(&target, name, &declared.source.artifact).await?;
            if installed.sha256 != declared.source.sha256 {
                return Err(CmdError::click(format!(
                    "{name}: declaration pins sha256 {} but the artifact installed {}",
                    declared.source.sha256, installed.sha256
                )));
            }
            if args.is_empty() {
                declaration_args = declared.run.args;
            }
            (installed.program_path.clone(), Some(installed))
        }
        (Some(_), Some(_)) => {
            return Err(CmdError::click("--from and --from-artifact are exclusive"))
        }
    };
    let from = from.as_str();
    let args: &[String] = if args.is_empty() {
        &declaration_args
    } else {
        args
    };
    let mut plan = match launchd_label {
        Some(label) => service::plan_deploy_labelled(&target, name, label, from, args, &[]),
        None => service::plan_deploy(&target, name, from, args),
    }
    .map_err(click)?;
    if as_launch_agent {
        plan.force_daemon = false;
    }

    // Refuse a colliding declaration BEFORE touching the host: pushing a
    // unit that then cannot be recorded would leave an unmanaged unit
    // running, which is the whole failure this command family closes.
    let declared = service::declared_services(&target);
    for taken in [name, plan.label.as_str(), plan.unit.as_str()] {
        if declared.iter().any(|candidate| candidate.matches(taken)) {
            return Err(CmdError::click(format!(
                "{host} already manages {taken}; retire it first"
            )));
        }
    }

    let runner = production_runner();
    let report = service::deploy_service(&target, &plan, &runner)
        .await
        .map_err(click)?;
    if !report.succeeded("deployed") {
        return Err(CmdError::click(format!(
            "{host}: could not deploy {name}: {}",
            report.failure()
        )));
    }

    let mut record =
        service::record_from_report(&host, host_heuristic.as_deref(), name, &report, &now());
    record.program = plan.program.clone();
    record.args = args.to_vec();
    let generation = match record_declaration(&record).await {
        Ok(generation) => generation,
        // The unit is on the host and running; only the declaration failed.
        // Reporting that as a bare registry error would leave exactly the
        // running-but-unmanaged state this command family closes, so say
        // what happened and name the one command that repairs it.
        Err(exc) => {
            let detail = exc
                .message
                .unwrap_or_else(|| "registry write failed".to_string());
            return Err(CmdError::click(format!(
                "{host}: {name} is deployed and running, but recording it failed: {detail}. \
                 Run `stado service adopt {} --host {host}` to bring it under management.",
                record.unit_id()
            )));
        }
    };
    // The version is the point of --from-artifact: without it the operator is
    // back to "something is deployed" with no way to say what. Reported beside
    // the unit rather than inside the remote report, which describes the host
    // action and not what was installed.
    if let Some(installed) = installed.as_ref() {
        if !as_json {
            println!(
                "installed {name} version {} (sha256 {})",
                installed.version, installed.sha256
            );
        }
    }
    render_mutation(
        "deployed",
        &record,
        &generation,
        Some(&report.to_json()),
        as_json,
    )
}

struct EnsureOptions<'a> {
    name: &'a str,
    host: &'a str,
    from: Option<&'a str>,
    args: &'a [String],
    env: &'a [String],
    reason: &'a str,
    as_daemon: bool,
    as_launch_agent: bool,
    as_json: bool,
}

/// What a unit runs, and which declaration said so.
///
/// `pub(crate)` because the autonomy service reconciler renders repair units
/// through this exact chain; a second resolution order over there is how one
/// unit gets two different programs depending on who asked.
pub(crate) struct UnitProgram {
    pub(crate) program: String,
    pub(crate) args: Vec<String>,
    /// `"flag"`, `"registry"`, `"catalog"` or `"shipped"`.
    pub(crate) source: &'static str,
    /// Stable unit identity supplied by a registry or catalog declaration.
    pub(crate) unit: Option<String>,
    /// Non-secret environment declared by this unit, with target placeholders intact.
    pub(crate) env: std::collections::BTreeMap<String, String>,
    /// Exact authored systemd definition retained by registry lifecycle repairs.
    pub(crate) systemd_unit: String,
}
pub(crate) fn declared_label(service: &ManagedService) -> Option<&str> {
    let unit_id = service.unit_id();
    Some(unit_id.strip_suffix(".service").unwrap_or(unit_id)).filter(|label| !label.is_empty())
}

/// Stable unit identity supplied by the managed-product declaration.
///
/// Service names are operator-facing leaves (`stado-resolver`), while the
/// product catalog carries the init system's full identity
/// (`com.wisent.stado-resolver`). A registry record may contain the broken
/// historical spelling, so it cannot be the authority for this lookup.
fn canonical_managed_unit(name: &str, target: &str) -> Result<Option<String>, CmdError> {
    let requested = name.strip_suffix(".service").unwrap_or(name);
    let mut matched: Option<String> = None;
    let products = crate::deploy::products::declared().map_err(click)?;
    for product in products {
        for unit in &product.units {
            let label = unit.label_for(target);
            let bare = label.strip_suffix(".service").unwrap_or(&label);
            let leaf = bare.rsplit('.').next().unwrap_or(bare);
            if label != name && bare != requested && leaf != requested {
                continue;
            }
            if matched.as_deref().is_some_and(|existing| existing != label) {
                return Err(CmdError::click(format!(
                    "managed product declarations give {name} more than one unit identity"
                )));
            }
            matched = Some(label);
        }
    }
    Ok(matched)
}

/// The program and argument vector `ensure` renders the unit from.
///
/// Flags win, because an operator correcting a wrong declaration has to be
/// able to. Absent them, the host's own `services[]` entry answers: a
/// declaration that carries its program is one this command can reinstall
/// from the document alone, which is the whole difference between a declared
/// service and a plist somebody installed by hand — the resolver and the
/// local dashboard on `operator-host` were the second kind, and nothing in
/// the product knew their restart policy. Last, the declaration shipped in
/// this build ([`targets::load_bundled_registry`]), which is how a unit
/// declared in a release reaches a canonical document published before it:
/// the first `ensure` writes it there.
pub(crate) fn unit_program(
    host: &str,
    name: &str,
    from: Option<&str>,
    args: &[String],
    declared: Option<&ManagedService>,
) -> Result<UnitProgram, CmdError> {
    if let Some(from) = from {
        return Ok(UnitProgram {
            program: from.to_string(),
            args: args.to_vec(),
            source: "flag",
            unit: None,
            env: declared
                .map(|service| service.env.clone())
                .unwrap_or_default(),
            systemd_unit: declared
                .map(|service| service.systemd_unit.clone())
                .unwrap_or_default(),
        });
    }
    if !args.is_empty() {
        return Err(CmdError::usage(
            "--arg needs --from: an argument vector without the program it belongs to would be \
             appended to a declared program the caller never named",
        ));
    }
    if let Some(declared) = declared.filter(|candidate| !candidate.program.is_empty()) {
        return Ok(UnitProgram {
            program: declared.program.clone(),
            args: declared.args.clone(),
            source: "registry",
            unit: Some(declared.unit_id().to_string()),
            env: declared.env.clone(),
            systemd_unit: declared.systemd_unit.clone(),
        });
    }
    // The shipped Wisent catalog answers by name, on any host, with no
    // declaration of the operator's own — that is the whole of "run Weles
    // here" as one word.
    if let Some(entry) = crate::deploy::service_catalog::lookup(name)
        .map_err(|error| CmdError::click(error.to_string()))?
    {
        return Ok(UnitProgram {
            // Placeholders survive here on purpose: `$HOME` and
            // `$STADO_PLATFORM` belong to the target, and only the caller
            // holding the resolved target may expand them.
            program: entry.program,
            args: entry.args,
            source: "catalog",
            unit: entry.unit,
            env: entry.env,
            systemd_unit: String::new(),
        });
    }
    let bundled =
        targets::load_bundled_registry().map_err(|error| CmdError::click(error.to_string()))?;
    let shipped = bundled
        .lookup(host)
        .map(service::declared_services)
        .unwrap_or_default()
        .into_iter()
        .find(|candidate| candidate.matches(name) && !candidate.program.is_empty());
    if let Some(shipped) = shipped {
        let unit = shipped.unit_id().to_string();
        return Ok(UnitProgram {
            program: shipped.program,
            args: shipped.args,
            source: "shipped",
            unit: Some(unit),
            env: shipped.env,
            systemd_unit: shipped.systemd_unit,
        });
    }
    Err(CmdError::usage(format!(
        "nothing declares what {name} runs on {host}: pass --from PATH (repeating --arg for each \
         argument), give its registry services[] entry a \"program\" and \"args\", or pick one of \
         the preconfigured Wisent services `stado service catalog` lists"
    )))
}

/// `service catalog`: the preconfigured Wisent services this build ships,
/// printed as they would deploy. Read-only and local: the answer comes from
/// the compiled-in document, never from a host.
async fn catalog(json: bool) -> Result<(), CmdError> {
    let entries = crate::deploy::service_catalog::all()
        .map_err(|error| CmdError::click(error.to_string()))?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "services": entries.iter().map(|entry| json!({
                    "name": entry.name,
                    "summary": entry.summary,
                    "program": entry.program,
                    "args": entry.args,
                })).collect::<Vec<_>>(),
            }))?
        );
    } else {
        for entry in &entries {
            println!(
                "{:<14} {} {}",
                entry.name,
                entry.program,
                entry.args.join(" ")
            );
            println!("{:<14} {}", "", entry.summary);
        }
    }
    Ok(())
}

/// Ensure one dependency on the machine running this CLI.
///
/// Release submission uses this before its first object write. Keeping the
/// call inside the binary means every caller gets the same repair; a workflow
/// cannot accidentally rely on a listener left behind by an earlier run.
pub(crate) async fn ensure_local_dependency(
    name: &str,
    reason: &str,
    as_daemon: bool,
) -> Result<(), CmdError> {
    let hostname = crate::providers::vast::system_hostname();
    let registry = super::registry::read_registry().await?;
    let host = registry
        .lookup_self(&hostname)
        .map_err(|error| CmdError::click(error.to_string()))?
        .map(|target| target.name.clone())
        .ok_or_else(|| {
            CmdError::click(format!(
                "cannot ensure {name}: this machine {hostname:?} is not a registry target"
            ))
        })?;
    ensure(EnsureOptions {
        name,
        host: &host,
        from: None,
        args: &[],
        env: &[],
        reason,
        as_daemon,
        as_launch_agent: false,
        as_json: false,
    })
    .await
}

/// Reload an existing managed service after its configuration changes.
///
/// Configuration values are cached for the process lifetime. Native restart
/// preserves the unit's current command, including adopted units that have no
/// catalog recipe from which `ensure` could rebuild them.
pub(crate) async fn reconcile_after_config_change(name: &str, host: &str) -> Result<(), CmdError> {
    restart(name, Some(host), None, None, false).await
}

/// A changed unit may own the registry API itself. Wait for an actual
/// authoritative read after activation, not merely the new process's PID.
/// Only reads are repeated; the host action and conditional write never are.
async fn registry_after_host_change() -> Result<(Value, String), CmdError> {
    let mut last_error = None;
    let ready = async {
        loop {
            match registry::fetch_versioned_document().await {
                Ok(snapshot) => return Ok(snapshot),
                Err(error) => {
                    let code = error.failure.unwrap_or_else(|| {
                        crate::failure::classify_message(
                            error.message.as_deref().unwrap_or_default(),
                        )
                    });
                    if !code.retryable() {
                        return Err(error);
                    }
                    last_error = Some(error);
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    };
    match tokio::time::timeout(std::time::Duration::from_secs(30), ready).await {
        Ok(result) => result,
        Err(_) => Err(last_error.unwrap_or_else(|| {
            CmdError::click("the registry did not answer within 30 seconds after unit activation")
                .stating(crate::failure::FailureCode::InfraDown)
        })),
    }
}

/// `service ensure NAME --host HOST [--from PATH] --reason WHY`.
///
/// The idempotent half of `deploy`, and the only one that works on an ssh
/// login with no Aqua session. Two facts decide everything it does, and both
/// come from the host: what the unit on the box declares it runs, and what the
/// process under it is actually running. See
/// [`crate::deploy::service::ensure_service`].
async fn ensure(options: EnsureOptions<'_>) -> Result<(), CmdError> {
    let reason = options.reason.trim();
    if reason.is_empty() {
        return Err(CmdError::usage(
            "--reason must say why this host has to run this unit; it is recorded beside the \
             registry document this command declares the unit in",
        ));
    }
    let target = host_channel::canonical_target(options.host)
        .await
        .map_err(click)?;
    let host = target.name.clone();
    if options.as_launch_agent && !target.release_platform.starts_with("darwin") {
        return Err(CmdError::click("--as-launch-agent is Darwin-only"));
    }

    // Resolve the operator-facing name against both declarations that may
    // supply a stable init-system identity. The service catalog owns authored
    // services; the managed-product catalog owns units that execute a delivered
    // product binary.
    let declared = service::declared_services(&target);
    let catalog_entry = crate::deploy::service_catalog::lookup(options.name)
        .map_err(|error| CmdError::click(error.to_string()))?;
    let catalog_unit = catalog_entry.as_ref().and_then(|entry| entry.unit.clone());
    let managed_unit = canonical_managed_unit(options.name, &target.name)?;
    let canonical_unit = match (catalog_unit, managed_unit) {
        (Some(service_unit), Some(product_unit)) if service_unit != product_unit => {
            return Err(CmdError::click(format!(
                "service and managed-product declarations disagree about {}: {} versus {}",
                options.name, service_unit, product_unit
            )));
        }
        (Some(unit), _) | (_, Some(unit)) => Some(unit),
        (None, None) => None,
    };
    let existing = declared.iter().find(|candidate| {
        candidate.matches(options.name)
            || canonical_unit
                .as_deref()
                .is_some_and(|unit| candidate.matches(unit))
    });
    // The catalog's environment is the product's own requirement for the
    // unit, so it applies whatever declared the program: a registry entry
    // adopted from a hand-installed plist names the same binary and still
    // needs the same variables. Program and args keep their resolution
    // order; only the environment is defaulted from the catalog.
    let mut unit_env: Vec<(String, String)> = catalog_entry
        .as_ref()
        .map(|entry| {
            crate::deploy::service_catalog::resolve_entry(
                entry,
                &crate::deploy::service_catalog::home_for(&target),
                Some(&target.release_platform),
                &target.name,
            )
            .2
        })
        .unwrap_or_default();
    let mut unit = unit_program(&host, options.name, options.from, options.args, existing)?;
    if unit.source == "catalog" {
        let entry = crate::deploy::service_catalog::CatalogService {
            name: options.name.to_string(),
            summary: String::new(),
            unit: unit.unit.clone(),
            program: unit.program.clone(),
            args: unit.args.clone(),
            env: unit.env.clone(),
        };
        let (program, args, env) = crate::deploy::service_catalog::resolve_entry(
            &entry,
            &crate::deploy::service_catalog::home_for(&target),
            Some(&target.release_platform),
            &target.name,
        );
        unit.program = program;
        unit.args = args;
        unit_env = env;
        eprintln!(
            "{host} declares no program for {}; rendering the unit from the Wisent service \
             catalog this build ships: {} {}",
            options.name,
            unit.program,
            unit.args.join(" ")
        );
    }
    if unit.source == "shipped" {
        eprintln!(
            "{host} declares no program for {}; rendering the unit from the declaration shipped \
             with this build: {} {}",
            options.name,
            unit.program,
            unit.args.join(" ")
        );
    }
    let home = crate::deploy::service_catalog::home_for(&target);
    let mut env_overrides = unit.env;
    for assignment in options.env {
        let (name, value) = assignment
            .split_once('=')
            .ok_or_else(|| CmdError::usage("--env requires NAME=VALUE"))?;
        env_overrides.insert(name.to_string(), value.to_string());
    }
    for (name, value) in env_overrides {
        let value = crate::deploy::service_catalog::resolve_word(
            &value,
            &home,
            Some(&target.release_platform),
            &target.name,
        );
        match unit_env.iter_mut().find(|(key, _)| key == &name) {
            Some((_, current)) => *current = value,
            None => unit_env.push((name, value)),
        }
    }
    // A canonical declaration wins, then the identity carried by the resolved
    // program, then the unit already declared on this host. The canonical
    // identity must win even when the registry supplies the program: otherwise
    // a stale doubled unit name is faithfully re-rendered forever.
    let plan = match canonical_unit
        .as_deref()
        .or(unit.unit.as_deref())
        .or_else(|| existing.and_then(declared_label))
    {
        Some(label) => service::plan_deploy_labelled(
            &target,
            options.name,
            label,
            &unit.program,
            &unit.args,
            &unit_env,
        ),
        None => service::plan_deploy_labelled(
            &target,
            options.name,
            &crate::deploy::local_install::label(service::DEPLOY_KIND, options.name),
            &unit.program,
            &unit.args,
            &unit_env,
        ),
    }
    .map_err(click)?;
    let mut plan = plan;
    if !unit.systemd_unit.is_empty() {
        if !target.release_platform.starts_with("linux") {
            return Err(CmdError::click(format!(
                "{} declares a systemd unit definition on non-Linux platform {}",
                options.name, target.release_platform
            )));
        }
        let definition = std::mem::take(&mut unit.systemd_unit);
        unit.systemd_unit =
            service::retain_systemd_unit(&mut plan, &definition, &unit_env, unit.source == "flag")
                .map_err(click)?;
    }
    // A declared path is the service's durable domain choice. In particular,
    // a LaunchAgent intentionally placed on an always-on Mac must not become
    // a daemon again when ensure or the autonomy reconciler runs later.
    if options.as_launch_agent
        || (existing
            .is_some_and(|declared| service::UnitDomain::from_path(&declared.path).is_per_login())
            && !options.as_daemon)
    {
        plan.force_daemon = false;
    } else {
        // The target default remains the safe answer for undeclared services,
        // and --as-daemon can still turn the system domain on explicitly.
        plan.force_daemon = plan.force_daemon || options.as_daemon;
    }

    // An existing declaration is not a refusal here, and that is the whole
    // difference from `deploy`: asserting a unit that is already declared and
    // already running is what makes this safe to run twice, or from a script.
    let already = declared.into_iter().find(|candidate| {
        candidate.matches(options.name)
            || candidate.matches(&plan.label)
            || candidate.matches(&plan.unit)
    });

    let runner = production_runner();
    let outcome = service::ensure_service(&target, &plan, &runner)
        .await
        .map_err(click)?;
    if !outcome.succeeded() {
        let mut detail = format!(
            "{host}: could not ensure {}: {}",
            options.name,
            outcome.report.failure()
        );
        if !outcome.report.postcondition_held() {
            // A unit that will not stay up on a host where the same program
            // already runs outside any unit is the four-day incident from the
            // other side: launchd is being asked for a port a disowned process
            // still holds.
            detail.push_str(
                ". `stado service list --unowned` names a process that may still hold its port",
            );
        }
        return Err(CmdError::click(detail));
    }

    let mut record = service::record_from_ensure(&host, options.name, &outcome, &now());
    record.program = unit.program;
    record.args = unit.args;
    record.env = unit_env.into_iter().collect();
    record.systemd_unit = unit.systemd_unit;
    let persisted = async {
        let recovered_snapshot = if outcome.changed() {
            Some(registry_after_host_change().await?)
        } else {
            None
        };
        let generation = match &already {
            // Declared, at the same file and running the same program, by the
            // registry: the document already says what this pass just confirmed,
            // so nothing is written to it.
            Some(existing)
                if existing.source == SOURCE_REGISTRY
                    && existing.path == record.path
                    && existing.kind == record.kind
                    && existing.program == record.program
                    && existing.args == record.args
                    && existing.env == record.env
                    && existing.systemd_unit == record.systemd_unit =>
            {
                None
            }
            // Declared by the registry at a different file. The system-domain
            // daemon path is not the per-login agent path, and a declaration
            // naming a file the host does not have is one no later command can
            // act on, so the declaration is corrected in one document write.
            Some(existing) if existing.source == SOURCE_REGISTRY => {
                // Expected generation: this read. `existing` and `record` were
                // decided before it — the unit was already ensured on the host —
                // so a lost race is reported instead of retried: re-running the
                // correction would replace a declaration this pass never saw.
                let (mut document, expected_generation) = match recovered_snapshot {
                    Some(snapshot) => snapshot,
                    None => registry::fetch_versioned_document().await?,
                };
                service::remove_service(&mut document, &host, existing.unit_id()).map_err(click)?;
                service::add_service(&mut document, &record).map_err(click)?;
                Some(registry::push_document_if(&document, &expected_generation).await?)
            }
            // Undeclared, or carried by the fixed host-recovery list. Both become
            // an explicit registry declaration, which for the recovery case is
            // exactly what `service adopt` is for.
            _ => Some(record_declaration(&record).await?),
        };

        // Recorded only when something actually changed. An audit trail that also
        // records the passes which changed nothing is one nobody reads.
        let audited = if outcome.changed() || generation.is_some() {
            Some(
                record_ensure_audit(&record, &outcome, &plan, reason, generation.as_deref())
                    .await?,
            )
        } else {
            None
        };
        Ok::<_, CmdError>(audited)
    }
    .await;
    let audited = persisted.map_err(|mut error| {
        let cause = error
            .message
            .as_deref()
            .unwrap_or("registry operation failed");
        error.failure = Some(
            error
                .failure
                .unwrap_or_else(|| crate::failure::classify_message(cause)),
        );
        error.message = Some(format!(
            "{host}: {} is running (action {}, pid {}), but recording the completed ensure \
             failed: {cause}. No host action was repeated.",
            options.name,
            outcome.action,
            outcome.pid.trim(),
        ));
        error.json = options.as_json;
        error
    })?;

    if options.as_json {
        // Exactly the contract's keys: a desktop client consumes this shape.
        // Where the record landed goes to stderr rather than into the object.
        if let Some(audited) = audited.as_deref() {
            eprintln!("audit record {audited}");
        }
        return print_json(&json!({
            "host": host,
            "name": record.name,
            "label": record.unit_id(),
            "domain": outcome.domain_word(),
            "action": outcome.action,
            "pid": outcome.pid.trim().parse::<u32>().ok(),
        }));
    }
    table::print(
        &["HOST", "SERVICE", "LABEL", "DOMAIN", "ACTION", "PID"],
        &[vec![
            host,
            record.name.clone(),
            record.unit_id().to_string(),
            outcome.domain_word().to_string(),
            outcome.action.clone(),
            dash(outcome.pid.trim()),
        ]],
    );
    if let Some(audited) = audited.as_deref() {
        println!("audit record {audited}");
    }
    Ok(())
}

/// Where the record of one `ensure` pass lives, relative to the canonical
/// registry document.
const ENSURE_AUDIT_PREFIX: &str = "service_audit";

/// Append the record of one `ensure` pass beside the state it changed.
///
/// This command exists to be run from a script, and a change nobody typed is a
/// change nobody remembers making: the reason the operator gave, what the host
/// did about it, and the registry generation it produced are written as one
/// create-only object through [`targets::RegistryStore::write_beside`] — beside
/// the registry document, because the registry is the state that changed and on
/// a GCS deployment the queue store is a different bucket entirely.
async fn record_ensure_audit(
    record: &ManagedService,
    outcome: &service::EnsureOutcome,
    plan: &service::DeployPlan,
    reason: &str,
    generation: Option<&str>,
) -> Result<String, CmdError> {
    let store = targets::RegistryStore::open()
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))?;
    let now = chrono::Utc::now();
    let body = serde_json::to_string_pretty(&json!({
        "action": outcome.action,
        "host": record.host,
        "service": record.name,
        "unit": record.unit_id(),
        "kind": record.kind,
        "path": record.path,
        "domain": outcome.domain_word(),
        "pid": outcome.pid.trim().parse::<u32>().ok(),
        "program": plan.program,
        "argv": plan.argv,
        "reason": reason,
        "registry_generation": generation,
        "recorded_at": now.to_rfc3339(),
        "actor": crate::cli::autonomy_cmd::actor(),
    }))?;
    // Timestamp first so one host's records sort by when they happened, and
    // compact rather than RFC-3339 because the key is also a file name on the
    // local-file backend.
    let key = format!(
        "{ENSURE_AUDIT_PREFIX}/{}/{}-{}.json",
        record.host,
        now.format("%Y%m%dT%H%M%S%.6fZ"),
        record.unit_id()
    );
    let (path, created) = store
        .write_beside(&key, &body)
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))?;
    if !created {
        return Err(CmdError::click(format!(
            "{path} already exists, so this pass was not recorded; an audit record is never \
             replaced"
        )));
    }
    Ok(path)
}

/// `datetime.now(timezone.utc).isoformat()` as every other writer in the
/// crate stamps it (`queue/leases.rs::now_iso`).
fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn render_mutation(
    action: &str,
    record: &ManagedService,
    generation: &str,
    remote: Option<&Value>,
    json: bool,
) -> Result<(), CmdError> {
    if json {
        let mut payload = serde_json::json!({
            "action": action,
            "service": record.to_json(),
            "registry_generation": generation,
        });
        if let Some(remote) = remote {
            payload["remote"] = remote.clone();
        }
        return print_json(&payload);
    }
    table::print(
        &[
            "ACTION",
            "HOST",
            "SERVICE",
            "UNIT",
            "KIND",
            "PATH",
            "GENERATION",
        ],
        &[vec![
            action.to_string(),
            record.host.clone(),
            record.name.clone(),
            record.unit_id().to_string(),
            record.kind.clone(),
            dash(&record.path),
            generation.to_string(),
        ]],
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Logs
// ---------------------------------------------------------------------------

async fn logs(name: &str, host: Option<&str>, lines: usize, json: bool) -> Result<(), CmdError> {
    let services = declared_matching(name, host).await?;
    let runner = production_runner();
    let mut tails: Vec<ServiceLog> = Vec::new();
    for declared in &services {
        let target = host_channel::canonical_target(&declared.host)
            .await
            .map_err(click)?;
        tails.push(
            service::tail_logs(&target, declared, lines, &runner)
                .await
                .map_err(click)?,
        );
    }

    if json {
        let payload: Vec<Value> = tails.iter().map(ServiceLog::to_json).collect();
        return print_json(&Value::Array(payload));
    }
    for tail in &tails {
        // A log body is not tabular; it is the file. Head each one so a
        // multi-host tail stays attributable.
        println!("\n== {} {} ({}) ==", tail.host, tail.unit, tail.origin);
        print!("{}", tail.body);
        if !tail.body.ends_with('\n') {
            println!();
        }
        // stderr is its own file under launchd, so it is its own section;
        // the origin names the file, or the reason there was nothing to
        // show ("absent in plist", "<path> (empty)").
        if let Some(error_origin) = &tail.error_origin {
            println!(
                "== {} {} stderr ({}) ==",
                tail.host, tail.unit, error_origin
            );
            if !tail.error_body.is_empty() {
                print!("{}", tail.error_body);
                if !tail.error_body.ends_with('\n') {
                    println!();
                }
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Env
// ---------------------------------------------------------------------------

async fn env(name: &str, host: Option<&str>, json: bool) -> Result<(), CmdError> {
    let services = declared_matching(name, host).await?;
    let runner = production_runner();
    let mut environments: Vec<ServiceEnv> = Vec::new();
    for declared in &services {
        let target = host_channel::canonical_target(&declared.host)
            .await
            .map_err(click)?;
        let unit = service::fetch_unit_file(&target, declared, &runner)
            .await
            .map_err(click)?;
        environments.push(service::unit_environment(&unit).map_err(click)?);
    }

    if json {
        let payload: Vec<Value> = environments.iter().map(ServiceEnv::to_json).collect();
        return print_json(&Value::Array(payload));
    }

    let cells: Vec<Vec<String>> = environments
        .iter()
        .flat_map(|environment| {
            environment.env.iter().map(|(key, value)| {
                vec![
                    environment.host.clone(),
                    environment.unit.clone(),
                    key.clone(),
                    value.clone(),
                ]
            })
        })
        .collect();
    table::print(&["HOST", "UNIT", "VARIABLE", "VALUE"], &cells);

    for environment in &environments {
        for file in &environment.environment_files {
            // The pointer, not the contents: reporting it is how the
            // operator learns this listing is partial.
            println!(
                "{} {}: also reads EnvironmentFile={file} (not shown)",
                environment.host, environment.unit
            );
        }
    }
    Ok(())
}

/// Resolve one artifact reference and place that exact version on the host.
///
/// The alias is resolved before anything is written, so what lands on disk is
/// an immutable version and the path names it. Verification happens on the
/// host against the digest the manifest declares: a download that does not
/// match never becomes a running unit, and the previous `current` is left
/// where it was.
async fn install_from_artifact(
    target: &crate::targets::ComputeTarget,
    name: &str,
    reference: &str,
) -> Result<crate::deploy::artifact_install::InstalledArtifact, CmdError> {
    let registry = crate::artifacts::ArtifactRegistry::new()
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))?;
    let parsed = crate::artifacts_models::ArtifactRef::parse(reference)?;
    let manifest = registry.resolve_manifest(&parsed).await?;
    let runner = production_runner();
    crate::deploy::artifact_install::install_artifact(target, name, &manifest, &runner)
        .await
        .map_err(click)
}

/// Install a release archive that is not in an object store yet.
///
/// The published route is `--from-artifact`, and it stays the durable one. This
/// exists because a bundle has to reach a host before the fleet has a store
/// both machines can read, and the alternative people reach for in that gap is
/// copying a file by hand onto a running service. The archive is streamed over
/// the approved channel, checksummed on the far side, unpacked into a version
/// directory named for its own digest, and `current` is relinked only after the
/// digest matches.
/// The paths a gzip-compressed release archive carries, in archive order.
///
/// Read locally, before anything is copied to a host: the cheapest moment to
/// learn that a bundle cannot satisfy the unit it is meant for.
fn archive_members(path: &str) -> Result<Vec<String>, CmdError> {
    let file = std::fs::File::open(path)
        .map_err(|error| CmdError::click(format!("cannot read archive {path}: {error}")))?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
    let entries = archive
        .entries()
        .map_err(|error| CmdError::click(format!("{path} is not a tar archive: {error}")))?;
    let mut members = Vec::new();
    for entry in entries {
        let entry = entry
            .map_err(|error| CmdError::click(format!("{path} could not be listed: {error}")))?;
        let path = entry.path().map_err(|error| {
            CmdError::click(format!("{path} holds an unreadable name: {error}"))
        })?;
        members.push(normalize_member(&path.to_string_lossy()));
    }
    Ok(members)
}

/// One archive path, comparable: no `./` prefix, no trailing slash.
fn normalize_member(raw: &str) -> String {
    raw.trim_start_matches("./")
        .trim_end_matches('/')
        .to_string()
}

/// Refuse an archive that does not carry the file the unit executes after the
/// installer adds its fixed `darwin-arm/` platform directory.
///
/// The unit's program is an absolute path through `current`, while
/// [`ARCHIVE_INSTALL_BODY`] extracts the archive inside
/// `version_dir/darwin-arm`. A unit running
/// `.../current/darwin-arm/stado` therefore needs a root archive member
/// `stado`, not a second `darwin-arm/stado` nesting. Both sides are named in
/// the refusal, because the useful sentence is the mismatch, not the fact of
/// one.
fn refuse_archive_without_program(program: &str, members: &[String]) -> Result<(), String> {
    let Some(relative) = program.split("/current/").nth(usize::from(true)) else {
        // A unit pinned to a version directory rather than `current` is a
        // different fault, reported by `follow_current`; there is no program
        // path to look for here and inventing one would refuse every archive.
        return Ok(());
    };
    let relative = normalize_member(relative);
    let Some(archive_relative) = relative.strip_prefix("darwin-arm/") else {
        return Err(format!(
            "refusing to relink `current`: the archive installer writes below \
             current/darwin-arm, but the unit runs current/{relative}"
        ));
    };
    if archive_relative.is_empty() || members.iter().any(|member| member == archive_relative) {
        return Ok(());
    }
    let mut held: Vec<&str> = members
        .iter()
        .filter(|member| !member.ends_with('/'))
        .map(String::as_str)
        .take(6)
        .collect();
    if members.len() > held.len() {
        held.push("…");
    }
    Err(format!(
        "refusing to relink `current`: the unit runs current/{relative}; the archive holds {}. \
         Pointing `current` at a tree without that file does not fail here - it fails at \
         launchd's next spawn, which cannot report why, and a KeepAlive job that cannot spawn \
         leaves its domain. Install an archive whose layout matches the unit's program, or \
         change the unit's program to a path this archive carries.",
        if held.is_empty() {
            "nothing".to_string()
        } else {
            held.join(", ")
        }
    ))
}

/// Extract beside the immutable version and confirm the declared executable
/// before atomically replacing `current`. Never remove an installed version
/// while extracting its replacement.
async fn install_from_archive(
    target: &crate::targets::ComputeTarget,
    directory: &str,
    path: &str,
    required: &str,
    runner: &crate::deploy::Runner,
) -> Result<(crate::deploy::artifact_install::InstalledArtifact, bool), CmdError> {
    let bytes = std::fs::read(path)?;
    let digest = {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        hex::encode(hasher.finalize())
    };
    let version = format!("sha256-{}", &digest[..usize::from(12u8)]);
    let staged = format!(".stado/.{directory}-{version}.tar.gz");

    if crate::deploy::host_channel::target_is_this_host(target) {
        let home = std::env::var("HOME")
            .map_err(|_| CmdError::click("HOME is not set, so the staging path is unknown"))?;
        let destination = std::path::Path::new(&home).join(&staged);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(path, destination)?;
    } else {
        let connection = host_channel::select_ssh_connection(target, runner)
            .await
            .map_err(click)?;
        let ssh_target = connection.destination;
        let prepare = host_channel::run_script(
            target,
            "set -euo pipefail\n/bin/mkdir -p \"$HOME/.stado\"\n/bin/chmod 700 \"$HOME/.stado\"\n",
            runner,
        )
        .await
        .map_err(click)?;
        if !prepare.ok() {
            return Err(CmdError::click(format!(
                "{}: cannot prepare the staging directory",
                target.name
            )));
        }
        let mut options = host_channel::ssh_options(ssh_target);
        options.pop();
        let mut argv = vec!["scp".to_string(), "-q".to_string()];
        argv.extend(options.into_iter().skip(usize::from(true)));
        argv.push(path.to_string());
        argv.push(format!("{ssh_target}:{staged}"));
        let key = crate::deploy::ssh_key::materialize(&target.name)
            .await
            .map_err(click)?;
        let argv = crate::deploy::ssh_key::add_identity(argv, &key).map_err(click)?;
        let copy = runner(crate::deploy::CommandSpec::new(argv))
            .await
            .map_err(CmdError::click)?;
        if !copy.ok() {
            return Err(CmdError::click(format!(
                "{}: cannot deliver the archive: {}",
                target.name,
                copy.detail()
            )));
        }
    }

    let script = format!(
        "set -euo pipefail\nname={}\nversion={}\nexpected={}\nstaged={}\nrequired={}\n{ARCHIVE_INSTALL_BODY}",
        crate::deploy::shlex_quote(directory),
        crate::deploy::shlex_quote(&version),
        crate::deploy::shlex_quote(&digest),
        crate::deploy::shlex_quote(&staged),
        crate::deploy::shlex_quote(required),
    );
    let output = host_channel::run_script(target, &script, runner)
        .await
        .map_err(click)?;
    if !output.ok() {
        return Err(CmdError::click(format!(
            "{}: {}",
            target.name,
            host_channel::last_error_line(&output, "the archive did not install")
        )));
    }
    let already_active = output
        .stdout
        .lines()
        .any(|line| line == "STADO_SERVICE_ARCHIVE already_active");
    Ok((
        crate::deploy::artifact_install::InstalledArtifact {
            program_path: format!("$HOME/.stado/services/{directory}/current"),
            version,
            sha256: digest,
        },
        already_active,
    ))
}

const ARCHIVE_INSTALL_BODY: &str = r#"
root="$HOME/.stado/services/$name"
version_dir="$root/$version"
archive="$HOME/$staged"
incoming="$root/.$version.incoming.$$"
link="$root/.current.new.$$"
trap 'rm -f "$archive" "$link"; rm -rf "$incoming"' EXIT

[ -s "$archive" ] || { printf '%s\n' 'delivered archive is missing or empty' >&2; exit 1; }
actual="$(/usr/bin/shasum -a 256 "$archive" | /usr/bin/awk '{print $1}')"
if [ "$actual" != "$expected" ]; then
  printf '%s\n' "digest mismatch: expected $expected, delivered $actual" >&2
  exit 1
fi

/bin/mkdir -p "$incoming/darwin-arm"
/usr/bin/tar -xzf "$archive" -C "$incoming/darwin-arm"
if [ ! -f "$incoming/$required" ] || [ ! -x "$incoming/$required" ]; then
  printf '%s\n' "archive does not carry the declared executable $required; current is unchanged" >&2
  exit 1
fi
if [ -e "$version_dir" ]; then
  if ! /usr/bin/diff -qr "$incoming" "$version_dir" >/dev/null; then
    printf '%s\n' "existing immutable version $version differs from its archive; current is unchanged" >&2
    exit 1
  fi
  [ -x "$version_dir/$required" ] || {
    printf '%s\n' "existing immutable version $version has no executable $required; current is unchanged" >&2
    exit 1
  }
  rm -rf "$incoming"
else
  /usr/bin/python3 - "$incoming" "$version_dir" <<'PY'
import os, sys
os.rename(sys.argv[1], sys.argv[2])
PY
fi

if [ -L "$root/current" ] &&
   [ "$(/usr/bin/readlink "$root/current")" = "$version_dir" ]; then
  trap - EXIT
  rm -f "$archive"
  printf '%s\n' 'STADO_SERVICE_ARCHIVE already_active'
  exit 0
fi

# `current` is a directory here on some hosts and a symlink on others; either
# way the previous one is kept beside the new version rather than deleted, so a
# rollback is a rename.
if [ -e "$root/current" ] && [ ! -L "$root/current" ]; then
  /bin/mv "$root/current" "$root/current.before-$version.$$"
fi
/bin/ln -s "$version_dir" "$link"
/usr/bin/python3 - "$link" "$root/current" <<'PY'
import os, sys
os.replace(sys.argv[1], sys.argv[2])
PY
trap - EXIT
rm -f "$archive"
printf '%s\n' "$version_dir"
"#;

const ROLLBACK_BODY: &str = r#"
root="$HOME/.stado/services/$name"
target="$root/$version"
[ -d "$target" ] || { printf '%s\n' "no version directory $version on this host" >&2; exit 1; }
if [ -e "$root/current" ] && [ ! -L "$root/current" ]; then
  /bin/mv "$root/current" "$root/current.replaced-$(/bin/date -u +%Y%m%dT%H%M%SZ)"
else
  rm -f "$root/current"
fi
/bin/ln -sfn "$target" "$root/.current.new"
/bin/mv -f "$root/.current.new" "$root/current"
printf '%s\n' "$target"
"#;

/// Make the unit execute through `current`, if it was rendered against a
/// version directory instead.
///
/// The program is taken from the declaration, never from the rendered unit
/// summary. That summary is the program AND its arguments in one string, so
/// asking it for a path filename answered "stado coordinator" for
/// `com.wisent.compute.service.stado-local-control-plane` on 2026-09-03 and
/// wrote that as the program, leaving the declared `coordinator` to be
/// appended a second time: the unit file came out as
/// `.../current/darwin-arm/stado coordinator coordinator`, an argv the binary
/// cannot parse. launchd happened to still hold the previous job, so the
/// coordinator kept dispatching and the mis-render waited for the next time
/// anything made launchd re-read that plist.
///
/// It never showed on a unit already running from `current`, because the
/// marker branch below returns before any of this. It fires on exactly the
/// units being MOVED onto a content-addressed package -- the operation this
/// function exists for.
///
/// Returns whether anything had to change.
async fn follow_current(
    target: &crate::targets::ComputeTarget,
    declared: &crate::deploy::service::ManagedService,
    directory: &str,
    runner: &crate::deploy::Runner,
) -> Result<bool, CmdError> {
    // What the unit file on the host actually says today, always read, because
    // the declaration being right is not evidence that the file is. Those two
    // drifted apart on 2026-09-03 -- the declaration named the package program
    // and the file said `stado coordinator coordinator` -- and an earlier
    // version of this function returned here without looking, because the
    // DECLARED path was already on `current` and there was seemingly nothing
    // to repoint. Nothing else in the deployment path compares the two, so
    // that file would have sat there until the next reload.
    let report = service::show_service(target, declared, runner)
        .await
        .map_err(click)?;
    // The summary is `<program> <args...>`, then ` (current -> ...)`.
    let rendered = report
        .detail
        .trim()
        .split(" (current ->")
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    // A declaration that renders the unit states its program on its own. Only
    // a declaration that merely points at a hand-installed plist has none, and
    // then the rendered summary is the only source there is -- read for its
    // path alone rather than for a filename that would carry the arguments
    // with it.
    let program = if declared.program.trim().is_empty() {
        rendered.split_whitespace().next().unwrap_or_default()
    } else {
        declared.program.trim()
    };
    let marker = format!("/services/{directory}/");
    let pinned_to_a_version = match program.split_once(&marker) {
        Some((_, rest)) => match rest.split_once('/') {
            Some((segment, _)) => segment != "current",
            None => false,
        },
        None => true,
    };
    // The file agrees with the declaration word for word and the declaration
    // is already on `current`: nothing to write.
    let declared_argv = if declared.program.trim().is_empty() {
        String::new()
    } else {
        let mut words = vec![program.to_string()];
        words.extend(declared.args.iter().cloned());
        words.join(" ")
    };
    if !pinned_to_a_version && (declared_argv.is_empty() || declared_argv == rendered) {
        return Ok(false);
    }
    let wanted = if let Some((root, rest)) = program.split_once(&marker) {
        let Some((segment, tail)) = rest.split_once('/') else {
            return Ok(false);
        };
        if segment == "current" {
            program.to_string()
        } else {
            format!("{root}{marker}current/{tail}")
        }
    } else {
        let executable = std::path::Path::new(program)
            .file_name()
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                CmdError::click(format!(
                    "{} runs {program:?}, which has no executable filename",
                    declared.unit_id()
                ))
            })?;
        format!(".stado/services/{directory}/current/darwin-arm/{executable}")
    };
    // The declared arguments that must follow the program, in the
    // declaration's own words and order. Only the tail travels: the program
    // itself is `$wanted`, which the remote side is what expands a
    // `$HOME`-relative path, so composing the expected vector there is the
    // only way the comparison sees the same absolute path the plist got.
    // Empty program means a declaration that only names a plist, and then
    // there is no declared vector to compare against.
    let expected_args = if declared.program.trim().is_empty() {
        String::new()
    } else {
        declared.args.join("\n")
    };
    let script = format!(
        "set -euo pipefail\nunit_path={}\nwanted={}\ncompare_argv={}\nexpected_args={}\n{REPOINT_BODY}",
        crate::deploy::shlex_quote(&declared.path),
        crate::deploy::shlex_quote(&wanted),
        if declared.program.trim().is_empty() {
            "0"
        } else {
            "1"
        },
        crate::deploy::shlex_quote(&expected_args),
    );
    let output = host_channel::run_script(target, &script, runner)
        .await
        .map_err(click)?;
    if !output.ok() {
        return Err(CmdError::click(format!(
            "{}: installed the version but could not point {} at current: {}",
            target.name,
            declared.unit_id(),
            host_channel::last_error_line(&output, "repoint failed")
        )));
    }
    Ok(true)
}

/// Repoint one unit's program, and refuse to leave behind a unit that cannot
/// be executed.
///
/// The write is checked because an unchecked one has already happened: on
/// 2026-09-03 this step rendered
/// `.../current/darwin-arm/stado coordinator coordinator` for the fleet's
/// coordinator, wrote it, and reported success. Nothing compared what it had
/// written against anything, so the only reason the fleet kept dispatching is
/// that launchd was still holding the previous job in memory; the file would
/// have taken effect at the next boot, bootout or reload, with no one left to
/// unwind it. A step that writes a unit now proves the unit it wrote: the
/// program is an existing executable file on this host, and the rendered
/// argument vector equals the declared one word for word. Either check
/// failing restores the file it found and exits non-zero, so the caller gets
/// a refusal instead of a landmine.
const REPOINT_BODY: &str = r#"
[ -f "$unit_path" ] || { printf '%s
' "no unit file at $unit_path" >&2; exit 1; }
case "$unit_path" in
  /Library/*) sudo_prefix="/usr/bin/sudo -n" ;;
  *) sudo_prefix="" ;;
esac
case "$wanted" in
  /*) ;;
  *) wanted="$HOME/$wanted" ;;
esac
backup="$(/usr/bin/mktemp)"
$sudo_prefix /bin/cat "$unit_path" > "$backup"
restore_and_fail() {
  $sudo_prefix /bin/cp "$backup" "$unit_path"
  rm -f "$backup"
  printf '%s
' "$1" >&2
  exit 1
}
if [ "${compare_argv:-0}" = "1" ]; then
  # The whole vector is rewritten from the declaration, not just its first
  # element. Setting element zero alone is what let a duplicated subcommand
  # survive a repoint: the program was corrected in place while the stale
  # extra word sat behind it, and the file stayed unrunnable.
  $sudo_prefix /usr/libexec/PlistBuddy -c "Delete :ProgramArguments" "$unit_path" >/dev/null 2>&1 || true
  $sudo_prefix /usr/libexec/PlistBuddy -c "Add :ProgramArguments array" "$unit_path"     || restore_and_fail "could not create ProgramArguments on $unit_path"
  $sudo_prefix /usr/libexec/PlistBuddy -c "Add :ProgramArguments: string $wanted" "$unit_path"     || restore_and_fail "could not set the program on $unit_path"
  if [ -n "${expected_args:-}" ]; then
    printf '%s\n' "$expected_args" | while IFS= read -r word; do
      [ -n "$word" ] || continue
      $sudo_prefix /usr/libexec/PlistBuddy -c "Add :ProgramArguments: string $word" "$unit_path" || exit 1
    done || restore_and_fail "could not set the declared arguments on $unit_path"
  fi
  $sudo_prefix /usr/libexec/PlistBuddy -c "Delete :Program" "$unit_path" >/dev/null 2>&1 || true
else
  $sudo_prefix /usr/libexec/PlistBuddy -c "Set :ProgramArguments:0 $wanted" "$unit_path"     || $sudo_prefix /usr/libexec/PlistBuddy -c "Set :Program $wanted" "$unit_path"     || restore_and_fail "could not set the program on $unit_path"
fi
rendered="$($sudo_prefix /usr/libexec/PlistBuddy -c 'Print :ProgramArguments' "$unit_path" 2>/dev/null | /usr/bin/sed -e '1d' -e '$d' -e 's/^ *//' | /usr/bin/grep -v '^$')"
[ -n "$rendered" ] || rendered="$($sudo_prefix /usr/libexec/PlistBuddy -c 'Print :Program' "$unit_path" 2>/dev/null)"
program="$(printf '%s
' "$rendered" | /usr/bin/sed -n '1p')"
[ -f "$program" ] || restore_and_fail "the unit would run $program, which is not a file on this host"
[ -x "$program" ] || restore_and_fail "the unit would run $program, which is not executable"
if [ "${compare_argv:-0}" = "1" ]; then
  if [ -n "${expected_args:-}" ]; then
    expected_argv="$(printf '%s\n%s' "$wanted" "$expected_args")"
  else
    expected_argv="$wanted"
  fi
  if [ "$rendered" != "$expected_argv" ]; then
    restore_and_fail "the rendered argument vector [$(printf '%s' "$rendered" | /usr/bin/tr '
' ' ')] does not match the declared one [$(printf '%s' "$expected_argv" | /usr/bin/tr '
' ' ')]"
  fi
fi
rm -f "$backup"
printf '%s
' "$wanted"
"#;

#[cfg(test)]
mod tests {
    use super::readiness_probe_script;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::process::Command;
    use std::time::Duration;

    /// Serve one fixed JSON body on 200 to every request that arrives, on a
    /// loopback port the kernel picks, until the handle is dropped. The probe
    /// reads through `curl`, so a real socket is the only honest fixture.
    struct Health {
        port: u16,
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl Health {
        fn serving(body: &'static str) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("loopback port");
            let port = listener.local_addr().expect("bound address").port();
            listener
                .set_nonblocking(true)
                .expect("polling accept so the thread can be stopped");
            let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let flag = std::sync::Arc::clone(&stop);
            let thread = std::thread::spawn(move || {
                while !flag.load(std::sync::atomic::Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => answer(stream, body),
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(20));
                        }
                        Err(_) => return,
                    }
                }
            });
            Self {
                port,
                stop,
                thread: Some(thread),
            }
        }

        fn url(&self) -> String {
            format!("http://127.0.0.1:{}/healthz", self.port)
        }
    }

    impl Drop for Health {
        fn drop(&mut self) {
            self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    fn answer(mut stream: TcpStream, body: &str) {
        let mut request = [0u8; 1024];
        let _ = stream.read(&mut request);
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
    }

    /// A loopback port nothing is listening on: bound, its address read, then
    /// released. `curl` refuses the connection there.
    fn dead_url() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback port");
        let port = listener.local_addr().expect("bound address").port();
        drop(listener);
        format!("http://127.0.0.1:{port}/healthz")
    }

    /// Run the probe the way the host runs it and return `(ok, stderr)`.
    fn probe(url: &str, expected: Option<&str>, seconds: u64) -> (bool, String) {
        let script = readiness_probe_script(url, expected, seconds);
        let output = Command::new("/bin/bash")
            .arg("-c")
            .arg(&script)
            .output()
            .expect("bash runs the generated probe");
        (
            output.status.success(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    }

    #[test]
    fn a_url_that_never_answered_is_never_reported_as_a_missing_version_field() {
        let (ok, said) = probe(&dead_url(), Some("0.5.60"), 2);
        assert!(!ok, "a dead port cannot be ready: {said}");
        assert!(said.contains("never answered"), "{said}");
        assert!(
            !said.contains("releaseVersion"),
            "the field contract was never tested here: {said}"
        );
        assert!(
            !said.contains("build.version"),
            "the field contract was never tested here: {said}"
        );
    }

    #[test]
    fn a_service_serving_the_wrong_release_is_named_with_both_versions() {
        let health = Health::serving(r#"{"ok":true,"releaseVersion":"0.5.55"}"#);
        let (ok, said) = probe(&health.url(), Some("0.5.60"), 2);
        assert!(!ok, "0.5.55 is not the required 0.5.60: {said}");
        assert!(said.contains("reported 0.5.55"), "{said}");
        assert!(said.contains("not the required 0.5.60"), "{said}");
        assert!(!said.contains("never answered"), "it answered: {said}");
    }

    #[test]
    fn a_service_publishing_no_identity_at_all_is_named_as_that() {
        let health = Health::serving(r#"{"ok":true,"source":"weles_api"}"#);
        let (ok, said) = probe(&health.url(), Some("0.5.60"), 2);
        assert!(
            !ok,
            "an answer without an identity is not readiness: {said}"
        );
        assert!(
            said.contains("reported neither releaseVersion nor build.version"),
            "{said}"
        );
        assert!(said.contains("0.5.60 was required"), "{said}");
        assert!(!said.contains("never answered"), "it answered: {said}");
    }

    #[test]
    fn the_expected_release_passes_through_either_field() {
        let flat = Health::serving(r#"{"ok":true,"releaseVersion":"0.5.60"}"#);
        let (ok, said) = probe(&flat.url(), Some("0.5.60"), 4);
        assert!(ok, "readiness must still pass on releaseVersion: {said}");

        let nested = Health::serving(r#"{"ok":true,"build":{"version":"0.5.60"}}"#);
        let (ok, said) = probe(&nested.url(), Some("0.5.60"), 4);
        assert!(ok, "readiness must still pass on build.version: {said}");
    }

    #[test]
    fn a_probe_that_requires_no_version_is_ready_on_the_first_answer() {
        let health = Health::serving(r#"{"ok":true}"#);
        let (ok, said) = probe(&health.url(), None, 4);
        assert!(ok, "no version was required: {said}");
    }

    #[test]
    fn the_timeout_budget_is_the_one_that_was_asked_for() {
        // The sentence carries the budget the caller declared, and the probe
        // spends it: the registry's weles-worker window is 90s and a rollout
        // that reported a shorter one would be reporting someone else's gate.
        let script = readiness_probe_script("http://127.0.0.1:8788/healthz", Some("0.5.60"), 90);
        assert!(script.contains("deadline=$((SECONDS + 90))"), "{script}");
        assert_eq!(
            script.matches("readiness timed out after 90s").count(),
            3,
            "every refusal states the same budget: {script}"
        );

        let started = std::time::Instant::now();
        let (ok, said) = probe(&dead_url(), Some("0.5.60"), 2);
        assert!(!ok, "{said}");
        assert!(
            started.elapsed() >= Duration::from_secs(2),
            "the probe returned before its budget elapsed: {:?}",
            started.elapsed()
        );
    }
}
