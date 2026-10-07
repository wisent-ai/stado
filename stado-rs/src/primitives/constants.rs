//! Centralized tunables for wisent-compute.
//!
//! Port of `stado/constants.py`. Numeric values are either MEASURED (computed
//! from live system state), DERIVED (from an external constraint or another
//! constant), or DESIGN (explicit operational trade-off).

// ---------------------------------------------------------------------------
// Disk
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Timers / telemetry
// ---------------------------------------------------------------------------

/// Capacity reservations: the hold a placed workload (a Jeden session, a
/// browser task) keeps on a host while it runs, subtracted from the host's
/// broadcast. Its holder renews it on the host's own publication period and
/// promises that plus its last round (`queue::capacity::reservations`).
pub const RESERVATION_SCHEMA_VERSION: u64 = 1;
/// `stado fleet needs`: the report's schema version.
pub const NEEDS_SCHEMA_VERSION: u64 = 1;
