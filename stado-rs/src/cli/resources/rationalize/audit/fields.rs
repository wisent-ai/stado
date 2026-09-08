//! Reading inventory JSON without trusting its shape, and the one constructor
//! every finding in this tree goes through. A missing or unparsable field
//! makes a candidate ineligible instead of guessing a value for it.

use chrono::{DateTime, Duration, Utc};
use serde_json::Value;

use crate::cli::resources::rationalize::Finding;

pub(super) fn old_enough(value: Option<&Value>, min_age_seconds: u64, now: DateTime<Utc>) -> bool {
    let Some(timestamp) = value.and_then(Value::as_str) else {
        return false;
    };
    let Ok(created) = DateTime::parse_from_rfc3339(timestamp) else {
        return false;
    };
    now.signed_duration_since(created.with_timezone(&Utc))
        >= Duration::seconds(min_age_seconds.min(i64::MAX as u64) as i64)
}

pub(super) fn string_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string()
}

pub(super) fn number_field(value: &Value, key: &str) -> Option<u64> {
    value.get(key).and_then(|number| {
        number
            .as_u64()
            .or_else(|| number.as_str()?.parse::<u64>().ok())
    })
}

pub(super) fn first_nonempty(values: &[String]) -> String {
    values
        .iter()
        .find(|value| !value.is_empty() && value.as_str() != "unknown")
        .cloned()
        .unwrap_or_default()
}

pub(super) fn resource_at(name: &str, location: &str) -> String {
    if location.is_empty() {
        name.to_string()
    } else {
        format!("{name}@{location}")
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn finding(
    id: &str,
    severity: &'static str,
    action: &'static str,
    confidence: &'static str,
    provider: &str,
    resource_type: &'static str,
    resource: impl Into<String>,
    reason: &str,
    evidence: Value,
) -> Finding {
    Finding {
        id: id.to_string(),
        severity,
        action,
        confidence,
        provider: provider.to_string(),
        resource_type,
        resource: resource.into(),
        reason: reason.to_string(),
        evidence,
        automatic: false,
    }
}
