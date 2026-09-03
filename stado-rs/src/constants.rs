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
    "/usr/bin/tar -xzf release.tar.gz && exec ./bin/stado release delivery-worker --request delivery-request.json";
/// Release qualification and delivery unblock declared fleet versions, so
/// routine batch work must not leave them at the zero-priority FIFO tail.
pub const RELEASE_JOB_PRIORITY: i64 = 90_000_000;

// ---------------------------------------------------------------------------
// Timers / telemetry
// ---------------------------------------------------------------------------

/// Main agent poll interval (latency vs. GCS API load trade-off).
pub const POLL_INTERVAL_S: u64 = 10;

/// Capacity broadcast staleness threshold.
pub const CAPACITY_STALE_SECONDS: u64 = 180;
/// Capacity broadcast heartbeat; always fresh before the stale threshold.
pub const CAPACITY_HEARTBEAT_INTERVAL_S: u64 = if POLL_INTERVAL_S > CAPACITY_STALE_SECONDS / 3 {
    POLL_INTERVAL_S
} else {
    CAPACITY_STALE_SECONDS / 3
};

/// Wall-clock budget for ONE store read an agent performs on a path where a
/// capacity broadcast is waiting behind it.
///
/// Two such paths exist, and both were unbounded.
///
/// `disk_cleanup::run_cleanup_once` resolves the canonical registry and the
/// live job ids before it takes the janitor lock. It no longer runs inside the
/// agent tick (see [`crate::providers::local::agent_janitor`]), but unbounded
/// it still wedges the pass that owns disk reclamation while holding the
/// cross-process janitor lock, and disk pressure is what closes claiming.
///
/// The agent tick itself then refreshes the model policy and re-reads its own
/// canonical registry target BEFORE its keep-alive capacity publication, on
/// the same store route.
///
/// That route is measurably slow, not hypothetically: on 2026-09-03 the
/// janitor's two reads took 639 s and 2,178 s, and this host's own publish log
/// shows gaps of 96 minutes (18:18 -> 19:54) against a
/// [`CAPACITY_STALE_SECONDS`] window of 180. All three fleet hosts sat at
/// `capacity_publication_stale` with live agents and 14 jobs went unclaimed
/// for 267 hours.
///
/// Every read bounded by this budget has a defined fail-safe degradation, so
/// a store too slow to answer costs one cleanup pass or one tick's worth of
/// policy freshness — never the host's liveness. At two reads per tick the
/// worst case is 40 s, inside one [`CAPACITY_HEARTBEAT_INTERVAL_S`].
pub const AGENT_STORE_READ_TIMEOUT_S: u64 = 20;

/// Ceiling on ONE text object read out of the store.
///
/// The timeout above bounds how long a read may wait. It does not bound how
/// much the read may bring back, and an object body is buffered whole in the
/// process that asked for it — so an unbounded read is unbounded memory as
/// well as unbounded time, and a budget that fires afterwards fires too late.
/// `charless-mac-mini` was measured on 2026-09-03 with 88 MB of free pages,
/// ~3 GB held by the compressor and 3.1M swap-outs after seven days of
/// uptime, and the agent's loop is the process performing these reads on that
/// host every tick.
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

// ---------------------------------------------------------------------------
// Sizing / capacity caches
// ---------------------------------------------------------------------------

/// DESIGN: max completed blobs to sample when building observed_* maps.
pub const COMPLETED_SAMPLE_CAP: usize = 6000;

/// DESIGN: cache TTL for observed VRAM/RAM maps.
pub const OBSERVED_MAP_TTL_S: u64 = 600;

/// DESIGN: staleness threshold for live capacity broadcasts.
pub const LIVE_CAPACITY_TTL_S: u64 = 180;
