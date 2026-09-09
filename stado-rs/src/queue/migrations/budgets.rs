//! The per-call bounds this repair is built around, and the bulk fan-out it
//! shares with `queue::copy`.

/// Python `BACKFILL_BATCH`.
pub const BACKFILL_BATCH: usize = 500;
/// Index entries one repair call may delete.
///
/// The same shape of bound as `BACKFILL_BATCH` and for the same reason: the
/// repair runs on a tick, and replacing an unbounded read cost with an
/// unbounded delete cost would be no improvement. At this size the 9,021
/// markers measured on 2026-09-03 clear in a handful of ticks.
pub const MARKER_PRUNE_PER_CALL: usize = 500;
/// Python `_DOWNLOAD_WORKERS`.
pub(super) const DOWNLOAD_WORKERS: usize = 10;

/// The same bulk fan-out under a crate-visible name, so `queue::copy` can
/// reuse this budget for its backend-to-backend pass instead of picking a
/// second concurrency number.
pub(crate) const BULK_WORKERS: usize = DOWNLOAD_WORKERS;
