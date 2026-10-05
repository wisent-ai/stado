//! Host health, link and recovery diagnostics, and the read-only probes
//! (`uptime`, `ping`, `gates`, `exec`, `inventory`) they share.

pub(in crate::cli::host) mod health;
pub(in crate::cli::host) mod probes;
pub(in crate::cli::host) mod recovery;

/// The beacon is fresh and nothing refused.
const LINK_HEALTHY: &str = "healthy";
/// Nothing has been heard from this host since the silence threshold.
const LINK_SILENT: &str = "silent";
/// Readers refused inside the window, or the host answers ssh while its own
/// beacon is stale.
const LINK_DEGRADED: &str = "degraded";

/// What a host publishes no path for. Never a fabricated `direct`: "we do not
/// know how this host is reachable" is the answer that sends an operator to
/// look, and a guess is the answer that does not.
const PATH_KIND_UNKNOWN: &str = "unknown";
const HOST_HEALTH_AUTH_UNAVAILABLE: &str = "host-health authorization unavailable";
const HOST_HEALTH_LOG_LINES: u32 = 80;
const OBJECT_API_SERVICE: &str = "stado-object-api";
