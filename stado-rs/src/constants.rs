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

/// Wall-clock budget for ONE of the janitor's pre-lock store reads.
///
/// `disk_cleanup::run_cleanup_once` runs inline in the agent's main loop, and
/// the capacity broadcast the fleet judges liveness by is published later in
/// that SAME iteration. So an unbounded store read there does not merely delay
/// a cleanup: it spends the broadcast's entire freshness budget, and it stalls
/// claiming, which lives in the same loop.
///
/// That is not hypothetical. On 2026-09-03 the janitor's two reads took 639 s
/// and 2,178 s against a slow object-store route, so one loop iteration lasted
/// 10 and 36 minutes against a [`CAPACITY_STALE_SECONDS`] window of 180. All
/// three fleet hosts sat at `capacity_publication_stale` with live agents, and
/// 17 jobs went unclaimed for 264 hours.
///
/// Two reads at this budget are 40 s worst case: inside one
/// [`CAPACITY_HEARTBEAT_INTERVAL_S`], and well inside the window that judges
/// it. Both reads already have a defined fail-safe degradation, so a store too
/// slow to answer costs the janitor one pass, never the agent's liveness.
pub const CLEANUP_INPUT_TIMEOUT_S: u64 = 20;

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
