//! Read registry-managed host health beacons through Stado.
//!
//! Port of `stado/monitor/host_health.py` (`load_host_health` +
//! `format_host_health`).
//!
//! DEVIATIONS from Python (deliberate):
//! - Python reports full GCS object metadata (created_at, etag, real
//!   size). The storage layer does not expose those, so `created_at` and
//!   `etag` are null and `size_bytes` is the downloaded content length. The
//!   `generation` comes from `read_text_versioned`, whose version IS the GCS
//!   generation — generation-pinned exactly like Python's
//!   `if_generation_match` download.
//!
//! Registry parity: like Python (`lookup(..., source="gcs")`) targets are
//! resolved from the canonical remote registry ONLY, via
//! [`crate::targets::fetch_registry_remote`] — no bundled substitute.
//! DEVIATION: a
//! fetch failure surfaces as [`HostHealthError::RegistryFetch`] instead of
//! Python's empty registry, so "the store is unreachable" no longer
//! reports as "that target does not exist". Tests inject a downloader
//! serving the bundled document.
//!
//! The components are the two halves the port already had: `load` reads a
//! beacon — where one lives, which slugs a target resolves to, and the
//! report or the failure that comes back — and `format` renders a loaded
//! report for an operator. The prefix both of them are spelled against
//! stays here. Every name a caller outside this module uses is re-exported
//! here, so `crate::monitor::host_health::<item>` resolves exactly as
//! before.

mod format;
mod load;

pub use format::format_host_health;
pub use load::{
    beacon_object_path, beacon_slugs, load_host_health, HostHealthError, HostHealthReport,
};

/// Beacon blob prefix (Python `HEALTH_PREFIX`).
pub const HEALTH_PREFIX: &str = "host_health";
