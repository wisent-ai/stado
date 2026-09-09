//! Pressure and scan coverage are separate from deletion eligibility.

use super::super::render::gib;
use serde_json::Value;

pub(super) fn verdict(deficit: Option<i64>, observed: bool, outside: i64) -> &'static str {
    match deficit {
        None => "undeclared",
        Some(0) => "holds",
        Some(_) if !observed => "unmeasured",
        Some(_) if outside > 0 => "uncovered",
        Some(_) => "declared",
    }
}

pub(super) fn detail(word: &str, need: Option<i64>, covered: i64, outside: i64) -> String {
    match word {
        "undeclared" => "this target declares no free-space watermark".to_string(),
        "holds" => "the host is at or above its declared low watermark".to_string(),
        "unmeasured" => format!(
            "{} below the target; the inventory could not be measured, so scan coverage and recoverable space are unknown",
            need.map(gib).unwrap_or_else(|| "unknown distance".to_string())
        ),
        _ => format!(
            "{} below the target; the measured inventory has {} inside declared scan roots and {} outside them. Scan coverage does not establish how much can be deleted; read the recorded cleaner results below",
            need.map(gib).unwrap_or_else(|| "unknown distance".to_string()),
            gib(covered), gib(outside)
        ),
    }
}

pub(super) fn janitor_detail(state: &Value, need: Option<i64>) -> String {
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
    let distance = need
        .map(|bytes| format!("; currently {} below the target", gib(bytes)))
        .unwrap_or_default();
    format!("pass at {at} ended {outcome}{change}{distance}; retained files and exhausted limits are reported per cleaner, not inferred from directory sizes")
}
