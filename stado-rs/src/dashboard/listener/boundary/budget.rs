//! How often a closed boundary may be revalidated inline.

use std::time::Duration;

/// The least time between two inline revalidations of one closed boundary.
/// Long enough that a fleet hammering a shut boundary produces one vault sweep
/// per cooldown rather than one per request, short enough that a transient
/// reset costs seconds of 503 instead of a privileged restart.
pub(crate) fn boundary_recheck_cooldown() -> Duration {
    Duration::from_secs(
        std::env::var("WC_DASHBOARD_BOUNDARY_RECHECK_SECONDS")
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .filter(|seconds| *seconds > 0)
            .unwrap_or(30),
    )
}
