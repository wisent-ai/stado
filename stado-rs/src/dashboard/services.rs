//! Read-only fleet-services projection for the dashboard.
//!
//! [`services_payload`] is the JSON half of the `Fleet services` section in
//! `web_view`: the same records `stado service list` renders (from
//! [`service::list_services`], beacons only), plus the two facts the page
//! needs to explain why a failed unit cannot be restarted from the browser —
//! which launchd domain the unit lives in, and whether the approved
//! unprivileged channel can restart it at all.

use serde_json::{json, Value};

use crate::deploy::service::ServiceStatus;

/// launchd's system domain: the unit is a LaunchDaemon under
/// `/Library/LaunchDaemons`, and bootstrapping it needs sudo.
pub const DOMAIN_SYSTEM: &str = "system";
/// A LaunchAgent at the machine-wide `/Library/LaunchAgents`, loaded into
/// whichever login session is active.
pub const DOMAIN_ANY_USER: &str = "any-user";
/// A LaunchAgent under one account's home `Library/LaunchAgents`.
pub const DOMAIN_USER: &str = "user";
/// The path matches none of the launchd layouts (a systemd unit, or a
/// hand-written path): no domain is claimed.
pub const DOMAIN_UNKNOWN: &str = "unknown";

/// The launchd domain a unit path belongs to, derived from the path alone.
///
/// Mirrors the shell case-split in `deploy/service.rs` (`/Library/
/// LaunchDaemons/*` is `system`, everything else is a per-login domain),
/// refined so the machine-wide and per-account agent directories read as
/// different domains.
pub fn domain(path: &str) -> &'static str {
    if path.starts_with("/Library/LaunchDaemons/") {
        DOMAIN_SYSTEM
    } else if path.starts_with("/Library/LaunchAgents/") {
        DOMAIN_ANY_USER
    } else if path.contains("/Library/LaunchAgents/") {
        // `$HOME/…`, `~/…`, `/Users/<account>/…` — the per-login domain.
        DOMAIN_USER
    } else {
        DOMAIN_UNKNOWN
    }
}

/// The `/api/services.json` body: one record per managed unit, the
/// [`ServiceStatus`] JSON plus `domain` and `restartable`.
pub fn services_payload(rows: &[ServiceStatus]) -> Value {
    rows.iter()
        .map(|row| {
            let mut record = row.to_json();
            let word = domain(&row.service.path);
            record["domain"] = json!(word);
            record["restartable"] = json!(word != DOMAIN_SYSTEM);
            record
        })
        .collect()
}
