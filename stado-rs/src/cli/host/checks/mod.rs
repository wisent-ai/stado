//! Host health, link and recovery diagnostics, and the read-only probes
//! (`uptime`, `ping`, `gates`, `exec`, `inventory`) they share.

pub(in crate::cli::host) mod health;
pub(in crate::cli::host) mod probes;
pub(in crate::cli::host) mod recovery;

/// How many silence records `stado host link` carries in its document.
///
/// Five, newest first: enough that a host which has been dropping off every
/// afternoon shows a pattern rather than a single incident, and few enough that
/// the document stays readable on a terminal during the outage it describes.
/// The full history stays in the store under `host_silence/<host>/`.
const NEWEST_SILENCES: usize = 5;

/// How far back `stado host link` counts what readers refused.
///
/// One hour rather than the silence threshold. The refusals a gap produces land
/// AROUND it, not inside it: on 2026-08-19 the resolver refused twice while the
/// beacon was still inside its tolerance, so a window as narrow as the
/// threshold would report the gap with none of the refusals it caused. An hour
/// is the span an operator asking "why did this host go quiet" has in mind, and
/// every refusal record keeps its own timestamp for any question longer than
/// that.
const REFUSAL_WINDOW_SECONDS: i64 = 60 * 60;

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
const HOST_HEALTH_BEACON_UNIT_MACOS: &str = "com.wisent.host-health-beacon";
const HOST_HEALTH_BEACON_UNIT_LINUX: &str = "stado-host-beacon.service";
const HOST_HEALTH_AUTH_UNAVAILABLE: &str = "host-health authorization unavailable";
const HOST_HEALTH_LOG_LINES: u32 = 80;
const OBJECT_API_SERVICE: &str = "stado-object-api";
const LINK_REPAIR_WAIT_SECONDS: u64 = 90;
const LINK_REPAIR_POLL_SECONDS: u64 = 5;
