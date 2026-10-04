//! The bulk fan-out this repair shares with `queue::copy`.

/// Python `_DOWNLOAD_WORKERS`.
pub(super) const DOWNLOAD_WORKERS: usize = 10;

/// The same bulk fan-out under a crate-visible name, so `queue::copy` can
/// reuse this budget for its backend-to-backend pass instead of picking a
/// second concurrency number.
pub(crate) const BULK_WORKERS: usize = DOWNLOAD_WORKERS;
