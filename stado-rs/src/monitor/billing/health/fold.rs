//! Carrying one provider's history forward, and rendering the elapsed
//! figures that fold produces.

use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use super::{
    ProviderHealth, HEALTH_GRACE_SECONDS, MISSING_STATUS, OK_STATUS, SECONDS_PER_DAY,
    SECONDS_PER_HOUR, SECONDS_PER_MINUTE,
};

/// Carry one provider's history forward against this tick's section.
pub(super) fn fold_provider(
    provider: &str,
    section: Option<&Value>,
    prior: Option<&Value>,
    stamp: &str,
    now: DateTime<Utc>,
) -> ProviderHealth {
    let status = section
        .and_then(|section| section.get("status"))
        .and_then(Value::as_str)
        .unwrap_or(MISSING_STATUS)
        .to_string();
    let detail = section
        .and_then(|section| section.get("detail"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let prior_str = |field: &str| {
        prior
            .and_then(|prior| prior.get(field))
            .and_then(Value::as_str)
            .map(str::to_string)
    };

    if status == OK_STATUS {
        return ProviderHealth {
            provider: provider.to_string(),
            status,
            detail,
            last_ok: Some(stamp.to_string()),
            failing_since: None,
            failing_seconds: i64::default(),
            degraded: false,
        };
    }
    // An already-open run keeps its original start, so the elapsed figure
    // survives restarts of whatever process happens to be collecting.
    let failing_since = prior_str("failing_since").unwrap_or_else(|| stamp.to_string());
    let failing_seconds = elapsed_seconds(&failing_since, now);
    ProviderHealth {
        provider: provider.to_string(),
        status,
        detail,
        last_ok: prior_str("last_ok"),
        failing_since: Some(failing_since),
        degraded: failing_seconds >= HEALTH_GRACE_SECONDS,
        failing_seconds,
    }
}

pub(super) fn health_value(health: &ProviderHealth) -> Value {
    json!({
        "status": health.status,
        "detail": health.detail,
        "last_ok": health.last_ok,
        "failing_since": health.failing_since,
        "failing_seconds": health.failing_seconds,
        "degraded": health.degraded,
    })
}

/// Seconds between an RFC-3339 stamp and `now`, floored at zero. An
/// unparseable stamp yields zero, which keeps a corrupt record quiet rather
/// than alert-storming on garbage.
fn elapsed_seconds(since: &str, now: DateTime<Utc>) -> i64 {
    DateTime::parse_from_rfc3339(since)
        .map(|start| {
            (now - start.with_timezone(&Utc))
                .num_seconds()
                .max(i64::default())
        })
        .unwrap_or_default()
}

/// Elapsed seconds as `1d 2h 3m`. Fed only by [`elapsed_seconds`], so the
/// input is already non-negative; a negative one degrades to `0m`.
pub fn humanize(seconds: i64) -> String {
    let total = u64::try_from(seconds).unwrap_or_default();
    let days = total / SECONDS_PER_DAY;
    let hours = (total % SECONDS_PER_DAY) / SECONDS_PER_HOUR;
    let minutes = (total % SECONDS_PER_HOUR) / SECONDS_PER_MINUTE;
    let mut parts = Vec::new();
    if days > u64::default() {
        parts.push(format!("{days}d"));
    }
    if hours > u64::default() {
        parts.push(format!("{hours}h"));
    }
    if minutes > u64::default() || parts.is_empty() {
        parts.push(format!("{minutes}m"));
    }
    parts.join(" ")
}
