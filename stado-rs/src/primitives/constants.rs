//! Centralized tunables for wisent-compute.
//!
//! Port of `stado/constants.py`. Numeric values are either MEASURED (computed
//! from live system state), DERIVED (from an external constraint or another
//! constant), or DESIGN (explicit operational trade-off).

// ---------------------------------------------------------------------------
// VRAM
// ---------------------------------------------------------------------------

/// DESIGN: hard VRAM safety buffer at admission. 5% of card VRAM, 4 GiB floor.
pub const VRAM_SAFETY_BUFFER_FRACTION: f64 = 0.05;
pub const VRAM_SAFETY_BUFFER_MIN_GB: u64 = 4;

/// DESIGN: RAM safety buffer. Same 5%-of-total / 4 GiB floor rule.
pub const RAM_SAFETY_BUFFER_FRACTION: f64 = 0.05;
pub const RAM_SAFETY_BUFFER_MIN_GB: u64 = 4;

// ---------------------------------------------------------------------------
// Disk
// ---------------------------------------------------------------------------

/// DESIGN: stale scratch/output dirs older than this are safe to evict.
pub const STALE_TRAINING_MAX_AGE_S: u64 = 3600;
/// Exact queue command for a signed Stado release delivery. The agent's
/// artifact resolver has already pinned `release.tar.gz` to its declared
/// digest; running that candidate's delivery worker lets a release repair an
/// older installed worker whose delivery semantics are the defect. This is the
/// only workload admitted while a host is below its disk watermark: it replaces
/// the agent binary that owns cleanup and admission.
pub const RELEASE_DELIVERY_JOB_COMMAND: &str =
    "/usr/bin/tar -xzf release.tar.gz && exec ./stado release delivery-worker --request delivery-request.json";
/// Exact queue command for delivering any product other than Stado. The
/// installed Stado worker verifies and applies the product archive; product
/// archives do not carry a second copy of Stado.
pub const PRODUCT_RELEASE_DELIVERY_JOB_COMMAND: &str =
    "exec $HOME/.stado/bin/stado release delivery-worker --request delivery-request.json";
/// Release qualification and delivery unblock declared fleet versions, so
/// routine batch work must not leave them at the zero-priority FIFO tail.
pub const RELEASE_JOB_PRIORITY: i64 = 90_000_000;
/// A detached agent session is work a person asked for and is waiting on, so
/// it must not sit behind a batch someone queued overnight; it must also
/// never outrank a release, which unblocks the fleet's declared versions.
pub const DETACHED_SESSION_JOB_PRIORITY: i64 = 50_000_000;
/// How long a platform the manifest marks `required: false` may sit in the
/// queue unclaimed before the release records it as unbuilt and finishes on
/// the platforms that are required.
///
/// It exists because a release with an optional platform nothing can claim
/// never ended. Skarbiec 0.3.5 published darwin-arm64 in twenty minutes and
/// then waited: its `linux-amd64` recipe reads the Skarbiec item
/// `browser-extension-key#private_key`, and the only Linux builder in this
/// fleet declines every job carrying workload secrets — "workload secrets
/// require a dedicated agent Skarbiec grant; leaving it queued for a host
/// that can resolve it", once a second, for hours. An optional platform is
/// exactly the case where waiting forever is the wrong answer: the operator
/// declared it skippable.
pub const OPTIONAL_PLATFORM_CLAIM_GRACE_S: u64 = 900;

// ---------------------------------------------------------------------------
// Timers / telemetry
// ---------------------------------------------------------------------------

/// Capacity broadcast staleness threshold.
pub const CAPACITY_STALE_SECONDS: u64 = 180;

/// Capacity reservations: the hold a placed workload (a Jeden session, a
/// browser task) keeps on a host while it runs, subtracted from the host's
/// broadcast. The TTL is three heartbeats, the same ratio the broadcast
/// itself uses: one missed heartbeat is a slow store, three is a holder that
/// is gone. GC keeps an expired row for an hour so an operator can still see
/// what just ended, and is capped per tick like the broadcast GC.
pub const RESERVATION_SCHEMA_VERSION: u64 = 1;
pub const RESERVATION_HEARTBEAT_SECONDS: u64 = 60;
pub const RESERVATION_TTL_SECONDS: u64 = RESERVATION_HEARTBEAT_SECONDS * 3;
pub const RESERVATION_GC_AGE_SECONDS: i64 = 3600;
pub const RESERVATION_GC_CAP_PER_TICK: usize = 200;
/// A host whose net capacity after reservations is below this many cores or
/// this much RAM publishes `accepting_jobs: false` with
/// `admission_reason: reservations_exhausted`.
pub const RESERVATION_MIN_FREE_CORES: i64 = 1;
pub const RESERVATION_MIN_FREE_RAM_GB: f64 = 1.0;

/// `stado fleet needs`: the advisor's own knobs. A queued job older than
/// ten minutes is demand the fleet is failing to serve; the advisor reads
/// the oldest 500 queued jobs; three refusals in the window make a host
/// "full" rather than momentarily busy; a host over its memory watermark is
/// suggested half again as much memory, and one whose swap is over its
/// watermark twice as much, because swap that high means the working set
/// already exceeds the box.
pub const NEEDS_SCHEMA_VERSION: u64 = 1;
pub const NEEDS_STALE_QUEUE_SECONDS: i64 = 600;
pub const NEEDS_QUEUE_WINDOW: usize = 500;
pub const NEEDS_REFUSALS_FOR_CPU: usize = 3;
pub const NEEDS_RAM_GROWTH_PRESSURE: f64 = 1.5;
pub const NEEDS_RAM_GROWTH_SWAP_OVER: f64 = 2.0;
pub const NEEDS_DEFAULT_WINDOW_DAYS: i64 = 7;

/// Ceiling on ONE text object read out of the store.
///
/// An object body is buffered whole in the process that asked for it, so an
/// unbounded read is unbounded memory.
/// A host under memory pressure can have tens of MB of free pages, GBs held
/// by the compressor and millions of swap-outs after a week of uptime, and
/// the agent's loop is the process performing these reads on that host every
/// tick.
///
/// Sized against what these documents are, not against what a host can
/// afford: the canonical registry is ~41 KB, a job is a few KB, a capacity
/// row and the queue-control record are smaller still. 16 MiB is several
/// hundred times the largest of them, so nothing legitimate is refused, and
/// a reply declaring more than this is refused before its body is requested.
/// Software artifacts do not read through here — they are unlimited and go
/// straight to a file.
pub const STORE_DOCUMENT_MAX_BYTES: usize = 16 * 1024 * 1024;

/// Per-job heartbeat interval (derived from the 15-min staleness threshold).
pub const SLOT_HEARTBEAT_INTERVAL_S: u64 = 60;

/// Fleet staging flush interval (~20 commits/hour, under the HF rate cap).
pub const FLEET_FLUSH_INTERVAL_S: u64 = 180;

/// Minimum runtime before a yieldable slot can be preempted again.
pub const MIN_RUNTIME_BEFORE_YIELD_S: u64 = 300;

/// Cache TTL for the CUDA child-probe in local_agent.
pub const CUDA_PROBE_CACHE_S: u64 = 30;

/// How long a janitor's request for its turn keeps new workloads from
/// claiming. Below its low watermark the janitor asks running workloads to
/// drain; a pass that asked and never ran must not keep the host from work
/// for longer than a release build takes to finish.
pub const CLEANUP_TURN_TTL_S: u64 = 1800;

// ---------------------------------------------------------------------------
// Sizing / capacity caches
// ---------------------------------------------------------------------------

/// DESIGN: max completed blobs to sample when building observed_* maps.
pub const COMPLETED_SAMPLE_CAP: usize = 6000;

/// DESIGN: cache TTL for observed VRAM/RAM maps.
pub const OBSERVED_MAP_TTL_S: u64 = 600;

/// DESIGN: staleness threshold for live capacity broadcasts.
pub const LIVE_CAPACITY_TTL_S: u64 = 180;
