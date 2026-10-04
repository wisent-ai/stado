//! The vocabulary this command reports in, and the one constant that decides
//! when a janitor counts as late.
//!
//! Every word here is reported VERBATIM: a blocker an operator reads has to
//! be greppable in the agent that published it, otherwise the CLI has
//! invented a second vocabulary for the same condition.

/// The agent's own word for "I cannot read the volume I write to, so I claim
/// nothing", published under this exact key in the capacity broadcast's
/// `diag`.
pub const DISK_PRESSURE_UNRESOLVED: &str = "disk_pressure_unresolved";

/// The agent's exact word for "the volume is at the disk-full threshold".
///
/// Unlike [`DISK_PRESSURE_UNRESOLVED`], this is a measured condition rather
/// than a missing reading. The janitor deletes everything the fleet put on
/// the host while it holds; the agent remains alive and accepts signed Stado
/// release deliveries so a newer binary can recover the host, but it refuses
/// release builds and ordinary queue work because those consume more disk.
pub const DISK_PRESSURE_ACTIVE: &str = "disk_pressure_active";

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
/// Reported before [`CAPACITY_PUBLICATION_STALE`]: a
/// host addressing a private store has no way to publish anything the control
/// plane will see, so its publication is stale by construction and the
/// staleness is downstream of this.
///
/// [`StorageReach::Device`]: crate::capabilities::StorageReach::Device
pub const AGENT_STORE_DEVICE_ONLY: &str = "agent_store_device_only";

/// This host answered with a storage backend this build has no adapter for, so
/// how far its agent's writes carry is not a thing this command can decide.
///
/// Both device-only storage and an unrecognized backend prevent validating
/// fleet reach, so both are blockers rather than informational notes. A
/// version difference between the host and caller can leave the caller
/// without an adapter for the host's backend.
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

/// This host has a disk attached that nothing has mounted.
///
/// A NOTE and never a blocker: the fleet writes to the volume under the
/// agent's home, and that volume's free space is the verdict above. It is
/// reported because an operator reading "29 GiB free" on a box they know to
/// hold terabytes has been told a true number about the wrong disk: a build
/// can be refused for want of room while a multi-terabyte disk sits attached
/// and unmounted. `stado space report <host>` names the device, its size and
/// its filesystem, if it has one.
pub const DISK_ATTACHED_UNMOUNTED: &str = "disk_attached_unmounted";

/// This host's janitor has not completed a pass within [`STALL_INTERVALS`]
/// times the disk-full rule's check cadence.
///
/// A BLOCKER while the volume is also full, and a note otherwise, and its own
/// condition rather than a shade of [`DISK_PRESSURE_ACTIVE`]: those two are
/// the disk being full and the mechanism that empties it being dead, they
/// fail at different times, and the second one is the one nothing else in
/// this product sees.
///
/// Two things this deliberately is not. It is not a pass that was PREVENTED:
/// a workload holds the run lock in shared mode for its whole duration and
/// every pass meanwhile answers `lock_busy`, which is the modelled answer and
/// not a fault, so a janitor being turned away is measured by
/// `last_prevented_at` and never accumulates here. And it does not refuse work
/// on a host that still has room: on a full volume a stalled janitor must
/// block, because nothing is bringing the space back; under the threshold,
/// refusing work creates no space and only removes capacity.
///
/// It has no exemption: every host runs the rule, so a janitor that has not
/// completed a pass in the window is late everywhere.
pub const DISK_CLEANUP_STALLED: &str = "disk_cleanup_stalled";

/// This host's janitor is being refused the run lock and has not completed a
/// pass within [`STALL_INTERVALS`] of the rule's check cadence: the lock is
/// not being taken turns with, it is HELD.
///
/// Its own word and not a shade of [`DISK_CLEANUP_STALLED`], because the two
/// send an operator to opposite places. A stalled janitor is a janitor that
/// ran and got nowhere — read its report, its policy, its errors. A held lock
/// is a janitor that never started, and the only thing worth looking at is the
/// process on the other end of `~/.cache/wisent-compute/disk-cleanup.lock`,
/// which `stado space report` names in `cleanup_lock.holders`.
///
/// A host can report the stalled word with room on its volume while its own
/// agent holds the lock, pointing an operator at a disk that is fine. The
/// mechanism —
/// [`crate::providers::local::slots::release_hold_for_exited_workload`] — is
/// fixed, and this word exists so the next hold that outlives its workload is
/// read as a lock and not as a full disk.
///
/// Blocks on the same rule as [`DISK_CLEANUP_STALLED`] and for the same
/// reason: under pressure a janitor that cannot run must refuse work, and
/// under the threshold refusing work creates no space. It is a note there.
pub const DISK_CLEANUP_LOCK_HELD: &str = "disk_cleanup_lock_held";

/// This host's agent cannot take the shared workload hold on the cleanup
/// lock because a standalone cleanup pass holds it exclusively, so every
/// claim stops before it starts. The agent's own word
/// ([`disk_cleanup::CLEANUP_IN_PROGRESS`]), published as `admission_reason`
/// in its capacity broadcast.
///
/// A BLOCKER whatever the disk says, unlike [`DISK_CLEANUP_STALLED`] and
/// [`DISK_CLEANUP_LOCK_HELD`], because it is not a prediction about space:
/// it is the claim path's own answer, read back. A janitor pass can hold the
/// lock for hours inside a directory macOS has parked behind a consent
/// dialog; every claim answers this word and takes nothing while the
/// publication still says `accepting_jobs: true`, and a queued release
/// delivery sits pinned to a host that cannot start it. The remedy is the
/// same as for the held lock: `stado space report` names the holder in
/// `cleanup_lock.holders`.
///
/// [`disk_cleanup::CLEANUP_IN_PROGRESS`]: crate::providers::local::disk_cleanup::CLEANUP_IN_PROGRESS
pub const CLEANUP_IN_PROGRESS: &str = crate::providers::local::disk_cleanup::CLEANUP_IN_PROGRESS;

/// How many of the rule's check intervals a janitor may miss before
/// [`DISK_CLEANUP_STALLED`] fires.
///
/// Four, because one missed pass is a lock this host lost to its own agent
/// tick and two is a registry read that timed out twice — both routine, both
/// self-correcting, and a gate that fires on them is a gate that gets muted.
/// Four consecutive misses is no longer weather.
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

/// This host publishes less headroom than the last build of the product and
/// platform being placed wrote as scratch: the build would take the volume to
/// the disk-full threshold, where the janitor deletes everything the fleet
/// put there — the build's own cache with it.
///
/// Not the agent's word: a host does not know what a build it has not run
/// will write. The coordinator reads the previous build's measured scratch
/// from the run store and judges the publication against it, because a build
/// pinned to a host with too little room dies of a full disk after half an
/// hour of compiling.
pub const RELEASE_SCRATCH_SHORT: &str = "release_scratch_short";
