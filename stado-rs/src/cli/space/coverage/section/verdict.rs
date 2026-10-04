//! The rule's verdict on the volume, and where the bytes sit. Scan coverage
//! is separate from deletion eligibility.

use super::super::render::gib;
use crate::providers::local::disk_cleanup::rule::{VolumeReading, DISK_FULL_PERCENT};
use serde_json::Value;

/// `unmeasured` without a volume reading, `below_threshold` under the rule's
/// threshold, `full` at or above it.
pub(super) fn verdict(reading: Option<VolumeReading>) -> &'static str {
    match reading {
        None => "unmeasured",
        Some(reading) if reading.full() => "full",
        Some(_) => "below_threshold",
    }
}

pub(super) fn detail(
    word: &str,
    reading: Option<VolumeReading>,
    fleet_bytes: i64,
    outside: i64,
) -> String {
    let used = reading
        .map(|reading| format!("{:.1}%", reading.used_percent()))
        .unwrap_or_else(|| "unknown".to_string());
    match (word, reading) {
        ("below_threshold", Some(reading)) => format!(
            "{used} used; {} more may be written before the volume reaches {DISK_FULL_PERCENT}% and the janitor deletes everything the fleet put here",
            gib(reading.headroom_bytes())
        ),
        ("full", Some(reading)) => format!(
            "{used} used, {} past the {DISK_FULL_PERCENT}% threshold; the measured inventory has {} in the fleet's areas and {} outside them, which is the user's and is never taken",
            gib(-reading.headroom_bytes()),
            gib(fleet_bytes),
            gib(outside)
        ),
        _ => "the volume reading is unavailable, so the rule's verdict cannot be given".to_string(),
    }
}

pub(super) fn janitor_detail(state: &Value) -> String {
    let Some(outcome) = state.get("outcome").and_then(Value::as_str) else {
        return "no completed janitor pass was recorded".to_string();
    };
    let at = state
        .get("last_pass_at")
        .and_then(Value::as_str)
        .unwrap_or("unknown time");
    let change = state
        .get("freed_bytes")
        .and_then(Value::as_i64)
        .map(|bytes| format!("; measured free-space change {}", gib(bytes)))
        .unwrap_or_default();
    format!("pass at {at} ended {outcome}{change}; retained files are reported per cleaner, not inferred from directory sizes")
}
