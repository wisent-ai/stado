//! Carrying one provider's history forward, and rendering the elapsed
//! figures that fold produces.

use std::num::NonZeroU64;

use chrono::{DateTime, TimeDelta, Utc};
use serde_json::{json, Value};

use super::{
    ProviderHealth, MISSING_STATUS, OK_STATUS, SECONDS_PER_DAY, SECONDS_PER_HOUR,
    SECONDS_PER_MINUTE,
};

/// A text field of a billing section or of the stored history, as written:
/// absent or null is none, text is itself, and anything else is shown in its
/// JSON form so the alert carries what the section held instead of dropping it.
fn text_field(value: Option<&Value>) -> Option<String> {
    match value {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) => Some(text.clone()),
        Some(other) => Some(other.to_string()),
    }
}

/// Carry one provider's history forward against this tick's section.
pub(super) fn fold_provider(
    provider: &str,
    section: Option<&Value>,
    prior: Option<&Value>,
    stamp: &str,
    now: DateTime<Utc>,
) -> ProviderHealth {
    // A section that names no status is the provider's own missing state.
    let status = match text_field(section.and_then(|section| section.get("status"))) {
        Some(status) => status,
        None => MISSING_STATUS.to_string(),
    };
    let mut detail = match text_field(section.and_then(|section| section.get("detail"))) {
        Some(detail) => detail,
        None => String::new(),
    };
    let prior_str = |field: &str| text_field(prior.and_then(|prior| prior.get(field)));
    // No time has passed at the start of a run, nor while a provider is healthy.
    let no_time = TimeDelta::zero().num_seconds();

    if status == OK_STATUS {
        return ProviderHealth {
            provider: provider.to_string(),
            status,
            detail,
            last_ok: Some(stamp.to_string()),
            failing_since: None,
            failing_seconds: no_time,
            degraded: false,
        };
    }
    // An already-open run keeps its original start, so the elapsed figure
    // survives restarts of whatever process happens to be collecting. A
    // recorded start that is not a time cannot be carried: the run is counted
    // from this tick and the detail says why, instead of reading as a run
    // that failed for no time at all.
    let (failing_since, failing_seconds) = match prior_str("failing_since") {
        None => (stamp.to_string(), no_time),
        Some(recorded) => match elapsed_seconds(&recorded, now) {
            Ok(seconds) => (recorded, seconds),
            Err(cause) => {
                detail.push_str(&format!(
                    " (the recorded failing_since {recorded:?} is not a time: {cause}; counted from now)"
                ));
                (stamp.to_string(), no_time)
            }
        },
    };
    ProviderHealth {
        provider: provider.to_string(),
        status,
        detail,
        last_ok: prior_str("last_ok"),
        failing_since: Some(failing_since),
        degraded: true,
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

/// Seconds between an RFC-3339 stamp and `now`, floored at none; a stamp
/// that is not RFC 3339 is the parser's error.
fn elapsed_seconds(since: &str, now: DateTime<Utc>) -> Result<i64, chrono::ParseError> {
    let start = DateTime::parse_from_rfc3339(since)?;
    Ok((now - start.with_timezone(&Utc)).max(TimeDelta::zero()).num_seconds())
}

/// Elapsed seconds as `1d 2h 3m`, the zero units left out and minutes always
/// shown when nothing larger is. A negative count, which only a clock that
/// stepped back can give, is said as such rather than read as none.
pub fn humanize(seconds: i64) -> String {
    let Ok(total) = u64::try_from(seconds) else {
        return format!("{seconds}s (the clock is behind the recorded start)");
    };
    let days = total / SECONDS_PER_DAY;
    let hours = (total % SECONDS_PER_DAY) / SECONDS_PER_HOUR;
    let minutes = (total % SECONDS_PER_HOUR) / SECONDS_PER_MINUTE;
    let mut parts = Vec::new();
    if NonZeroU64::new(days).is_some() {
        parts.push(format!("{days}d"));
    }
    if NonZeroU64::new(hours).is_some() {
        parts.push(format!("{hours}h"));
    }
    if NonZeroU64::new(minutes).is_some() || parts.is_empty() {
        parts.push(format!("{minutes}m"));
    }
    parts.join(" ")
}
