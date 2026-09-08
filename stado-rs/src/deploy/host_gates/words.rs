//! The vocabulary this command reports in, and the one constant that decides
//! when a janitor counts as late.
//!
//! Every word here is reported VERBATIM: a blocker an operator reads has to
//! be greppable in the agent that published it, otherwise the CLI has
//! invented a second vocabulary for the same condition.

/// The agent's own word for "I cannot prove there is room, so I claim
/// nothing": [`disk_cleanup::disk_pressure_unresolved`], published under this
/// exact key in the capacity broadcast's `diag`.
///
/// [`disk_cleanup::disk_pressure_unresolved`]: crate::providers::local::disk_cleanup::disk_pressure_unresolved
pub const DISK_PRESSURE_UNRESOLVED: &str = "disk_pressure_unresolved";

/// The agent's exact word for "the disk is below its low watermark".
///
/// Unlike [`DISK_PRESSURE_UNRESOLVED`], this is a measured condition rather
/// than a missing reading. The agent remains alive and accepts signed Stado
/// release deliveries so a newer binary can recover the host, but it refuses
/// release builds and ordinary queue work because those consume more disk.
pub const DISK_PRESSURE_ACTIVE: &str = "disk_pressure_active";

/// The agent publishes `disk_cleanup_policy_known: false` when it has no
/// validated low watermark at all. Reported as its negation because a blocker
/// list reads as a list of things that are wrong; the underlying key, and the
/// only place the value comes from, is still the agent's.
///
/// It never appears alone: an unknown threshold also makes
/// [`DISK_PRESSURE_UNRESOLVED`] true by that function's own truth table. It is
/// named separately because the two send an operator to different places — one
/// is a full disk, the other is a host that cannot read its own policy.
pub const DISK_CLEANUP_POLICY_UNKNOWN: &str = "disk_cleanup_policy_unknown";

/// `stado queue pause` is in effect; the agent publishes this per tick.
pub const QUEUE_PAUSED: &str = "queue_paused";

/// The registry pinned this host, so it claims only work addressed to it.
pub const PINNED_ONLY: &str = "pinned_only";

/// No capacity publication for this host exists at all. Not the agent's word,
/// because a silent agent has no words: the scheduler cannot see this host, so
/// it cannot be given anything.
pub const NO_CAPACITY_PUBLICATION: &str = "no_capacity_publication";

/// A publication older than [`capacity::CAPACITY_STALE_SECONDS`], which every
/// live-capacity reader in the fleet filters out. The row is reported anyway,
/// with its age, because "the agent said this an hour ago" and "nobody ever
/// said anything" are different findings.
///
/// [`capacity::CAPACITY_STALE_SECONDS`]: crate::queue::capacity::CAPACITY_STALE_SECONDS
pub const CAPACITY_PUBLICATION_STALE: &str = "capacity_publication_stale";

/// This host's queue agent is bound to a storage backend whose coordinates
/// carry only as far as the machine that wrote them
/// ([`StorageReach::Device`]), so everything it publishes — its capacity
/// broadcast, its claims, its view of the registry — lands in a store no other
/// host in the fleet can address.
///
/// Neither the agent's word nor the registry's: an agent writing into a
/// device-local store does not know it is alone, and this is the one blocker
/// in this list that the host cannot report about itself. The condition is
/// [`crate::capabilities::StorageReach`]'s, resolved from the host's own
/// effective `wc_storage_backend` read over the same channel `stado host
/// config-show` uses.
///
/// The incident: the Mac mini's agent unit was re-declared with a
/// `STADO_CONFIG` pointing at a config that set `wc_storage_backend` to
/// `local`, so the agent bound its `JobStorage` to `~/.stado/local-storage` on
/// that one machine. It kept ticking, kept reading a `registry.json` out of
/// that private store — a stale 20 GiB watermark against a canonical 15 —
/// kept computing [`DISK_PRESSURE_UNRESOLVED`] against it, and kept publishing
/// capacity nothing in the fleet could ever read. Seventy-four jobs, fifty-five
/// of them pinned to that host, sat in the fleet queue for days. Every surface
/// in this CLI reported the host in-sync, and this command — the one command
/// written to answer "why is this host claiming nothing" — could say only
/// [`CAPACITY_PUBLICATION_STALE`], which was true and was a symptom.
///
/// Reported BEFORE [`CAPACITY_PUBLICATION_STALE`] for exactly that reason: a
/// host addressing a private store has no way to publish anything the control
/// plane will see, so its publication is stale by construction and the
/// staleness is downstream of this.
///
/// [`StorageReach::Device`]: crate::capabilities::StorageReach::Device
pub const AGENT_STORE_DEVICE_ONLY: &str = "agent_store_device_only";

/// This host answered with a storage backend this build has no adapter for, so
/// how far its agent's writes carry is not a thing this command can decide.
///
/// A blocker and not a note, and deliberately: the two cases where a host's
/// store cannot be shown to be the fleet's are "it demonstrably is not"
/// ([`AGENT_STORE_DEVICE_ONLY`]) and "this control plane cannot tell", and the
/// second one is how the first one gets missed for a week. It usually means
/// the host is running a newer or older Stado than the machine asking.
pub const AGENT_STORE_UNKNOWN: &str = "agent_store_unknown";

/// The host did not answer with a storage backend at all — the remote `config
/// show` failed, or its output carried no `wc_storage_backend`.
///
/// A NOTE and never a blocker: the disk read and the capacity read both
/// succeeded to get this far, so the verdict this command reports is still the
/// verdict, and a store read that could not be taken is a gap in the
/// diagnosis rather than a reason the host claims nothing. It is reported
/// because the alternative is a report that silently omits the store line and
/// reads as "the store is fine".
pub const AGENT_STORE_UNREADABLE: &str = "agent_store_unreadable";

/// Local APFS snapshots are holding space while this host cannot prove it has
/// room.
///
/// A NOTE and never a blocker: snapshots do not stop the agent claiming, disk
/// pressure does. It is reported because an operator may next run `stado space
/// reclaim`; the declared `local_apfs_snapshots` stage can thin local Time
/// Machine snapshots to the target watermark, while OS-update snapshots remain
/// recovery state. macOS publishes no per-snapshot size
/// ([`host_disk::LocalSnapshots`]), so this says how many there are and never
/// how large they are.
///
/// [`host_disk::LocalSnapshots`]: crate::deploy::host_disk::LocalSnapshots
pub const LOCAL_SNAPSHOTS_UNRECLAIMABLE: &str = "local_snapshots_unreclaimable";

/// This host's janitor has not completed a pass within
/// [`STALL_INTERVALS`] times its own declared `check_interval_seconds`.
///
/// A BLOCKER while the disk is also under pressure, and a note otherwise, and
/// its own condition rather than a shade of [`DISK_PRESSURE_UNRESOLVED`]:
/// those two are the disk being full and the mechanism that empties it being
/// dead, they fail at different times, and the second one is the one nothing
/// in this product could see. On `lukasz-macbook` the janitor logged 12,197
/// passes between 2026-08-18 and 2026-09-02 and deleted nothing in any of
/// them — 8,539 never resolved a policy and 2,030 never got the run lock — so
/// `last_success_at` stayed null for fifteen days while every gate in the
/// fleet read green. The host then crossed its low watermark, releases stopped
/// fleet-wide, and the space came back by hand at one in the morning.
///
/// Two things this deliberately is not. It is not a pass that was PREVENTED:
/// a workload holds the run lock in shared mode for its whole duration and
/// every pass meanwhile answers `lock_busy`, which is the modelled answer and
/// not a fault, so a janitor being turned away is measured by
/// `last_prevented_at` and never accumulates here. And it does not refuse work
/// on a host that still has its headroom: below the watermark a stalled
/// janitor must block, because nothing is bringing the space back and
/// admitting a job is how the incident above ended; above it, refusing work
/// creates no space and only removes capacity.
///
/// Hosts that declare `mode: "off"` are exempt — a janitor nobody armed is not
/// a janitor that stalled — and so is a host with no declared interval to be
/// late against, which [`DISK_CLEANUP_POLICY_UNKNOWN`] already reports.
pub const DISK_CLEANUP_STALLED: &str = "disk_cleanup_stalled";

/// This host's janitor is being refused the run lock and has not completed a
/// pass within [`STALL_INTERVALS`] of its own declared interval: the lock is
/// not being taken turns with, it is HELD.
///
/// Its own word and not a shade of [`DISK_CLEANUP_STALLED`], because the two
/// send an operator to opposite places. A stalled janitor is a janitor that
/// ran and got nowhere — read its report, its policy, its errors. A held lock
/// is a janitor that never started, and the only thing worth looking at is the
/// process on the other end of `~/.cache/wisent-compute/disk-cleanup.lock`,
/// which `stado space report` names in `cleanup_lock.holders`.
///
/// On 2026-09-03 charless-mac-mini reported the stalled word with 18.4 GiB
/// free against a 15 GiB watermark while its own agent (pid 79473) held the
/// lock, and lukasz-macbook reported it with 118.7 GiB free against 100. Both
/// pointed at a disk that was fine. The mechanism —
/// [`crate::providers::local::slots::release_hold_for_exited_workload`] — is
/// fixed, and this word exists so the next hold that outlives its workload is
/// read as a lock and not as a full disk.
///
/// Blocks on the same rule as [`DISK_CLEANUP_STALLED`] and for the same
/// reason: under pressure a janitor that cannot run must refuse work, and
/// above the watermark refusing work creates no space. It is a note there.
pub const DISK_CLEANUP_LOCK_HELD: &str = "disk_cleanup_lock_held";

/// How many of its own check intervals a janitor may miss before
/// [`DISK_CLEANUP_STALLED`] fires.
///
/// Four, because one missed pass is a lock this host lost to its own agent
/// tick and two is a registry read that timed out twice — both routine, both
/// self-correcting, and a gate that fires on them is a gate that gets muted.
/// Four consecutive misses is no longer weather: at the hourly interval this
/// fleet declares it is a janitor that has been silent for half a working
/// day, and at the ten-second agent tick it is forty seconds.
pub(in crate::deploy::host_gates) const STALL_INTERVALS: i64 = 4;

/// The word this command reports when a host declares a queue agent and its
/// newest health beacon does not report that unit running.
///
/// Not the agent's word either — a unit that was never loaded publishes
/// nothing — but the registry's and the beacon's, joined. It exists because
/// [`NO_CAPACITY_PUBLICATION`] states only that a host is silent, and the
/// commonest cause of that silence in this fleet is a declaration naming a
/// unit no launchd or systemd on that host is running.
pub const AGENT_DECLARED_NOT_LOADED: &str = "agent_declared_not_loaded";
