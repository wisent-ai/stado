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

// ---------------------------------------------------------------------------
// Timers / telemetry
// ---------------------------------------------------------------------------

/// Main agent poll interval (latency vs. GCS API load trade-off).
pub const POLL_INTERVAL_S: u64 = 10;

/// Capacity broadcast staleness threshold.
pub const CAPACITY_STALE_SECONDS: u64 = 180;
/// Capacity broadcast heartbeat; always fresh before the stale threshold.
pub const CAPACITY_HEARTBEAT_INTERVAL_S: u64 =
    if POLL_INTERVAL_S > CAPACITY_STALE_SECONDS / 3 { POLL_INTERVAL_S } else { CAPACITY_STALE_SECONDS / 3 };

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
