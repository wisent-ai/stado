//! The rule's verdict on the volume, and where the bytes sit. Scan coverage
//! is separate from deletion eligibility.

use super::super::render::gib;
use crate::providers::local::space_reclaim::rule::{VolumeReading, DISK_FULL_PERCENT};
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
    incomplete: Option<&Value>,
    unmeasured: Option<&str>,
) -> String {
    match (word, reading) {
        ("below_threshold", Some(reading)) => format!(
            "{:.1}% used; {} more may be written before the volume reaches {DISK_FULL_PERCENT}% and the janitor deletes everything the fleet put here",
            reading.used_percent(),
            gib(reading.headroom_bytes())
        ),
        // An inventory that did not walk the volume measured only part of it:
        // its outside figure is what it happened to see, not what is there.
        ("full", Some(reading)) => match incomplete {
            Some(why) => format!(
                "{:.1}% used, {} past the {DISK_FULL_PERCENT}% threshold; the measured inventory has {} in the fleet's areas, and how much lies outside them was not measured: {}",
                reading.used_percent(),
                gib(-reading.headroom_bytes()),
                gib(fleet_bytes),
                // A text cause prints as its words; any other shape as JSON.
                match why.as_str() {
                    Some(text) => text.to_string(),
                    None => why.to_string(),
                }
            ),
            None => format!(
                "{:.1}% used, {} past the {DISK_FULL_PERCENT}% threshold; the measured inventory has {} in the fleet's areas and {} outside them, which is the user's and is never taken",
                reading.used_percent(),
                gib(-reading.headroom_bytes()),
                gib(fleet_bytes),
                gib(outside)
            ),
        },
        _ => match unmeasured {
            Some(cause) => format!("the volume is unmeasured, so the rule's verdict cannot be given: {cause}"),
            None => format!("the rule's verdict {word:?} does not match the volume reading this report holds"),
        },
    }
}

pub(super) fn janitor_detail(state: &Value) -> String {
    let Some(outcome) = state.get("outcome").and_then(Value::as_str) else {
        return "no completed janitor pass was recorded".to_string();
    };
    let at = match state.get("last_pass_at").and_then(Value::as_str) {
        Some(at) => at.to_string(),
        None => "a time the janitor did not record".to_string(),
    };
    // A pass that recorded no freed_bytes measured no change; the sentence
    // then carries none rather than a figure the janitor did not write.
    let change = match state.get("freed_bytes").and_then(Value::as_i64) {
        Some(bytes) => format!("; measured free-space change {}", gib(bytes)),
        None => String::new(),
    };
    format!("pass at {at} ended {outcome}{change}; retained files are reported per cleaner, not inferred from directory sizes")
}
