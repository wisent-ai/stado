//! Report fields, in the words a person reads.
//!
//! Split out of the command surface so the dispatcher stays the shape of the
//! contract — one arm per subcommand — and so the one piece of arithmetic here,
//! turning seconds left into a span, has a single home.

use serde_json::{Map, Value};

use crate::cli::CmdError;

pub(super) fn print_json(value: &Value) -> Result<(), CmdError> {
    let text = serde_json::to_string_pretty(value)
        .map_err(|exc| CmdError::click(format!("report is not serializable: {exc}")))?;
    println!("{text}");
    Ok(())
}

pub(super) fn text(report: &Map<String, Value>, key: &str) -> String {
    report
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

pub(super) fn count(report: &Map<String, Value>, key: &str) -> String {
    report
        .get(key)
        .and_then(Value::as_u64)
        .map_or_else(|| "none".to_string(), |value| value.to_string())
}

pub(super) fn names(report: &Map<String, Value>, key: &str) -> Vec<String> {
    report
        .get(key)
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

pub(super) fn objects<'a>(
    report: &'a Map<String, Value>,
    key: &str,
) -> Vec<&'a Map<String, Value>> {
    report
        .get(key)
        .and_then(Value::as_array)
        .map(|entries| entries.iter().filter_map(Value::as_object).collect())
        .unwrap_or_default()
}

pub(super) fn field(row: &Map<String, Value>, key: &str) -> String {
    match row.get(key) {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Bool(value)) => value.to_string(),
        Some(Value::Number(value)) => value.to_string(),
        _ => String::new(),
    }
}

pub(super) fn flag(row: &Map<String, Value>, key: &str) -> bool {
    row.get(key).and_then(Value::as_bool).unwrap_or_default()
}

/// How long one lease has left, in the operator's words rather than seconds:
/// a negative number is the thing an operator has to translate, and translating
/// it wrong is how an expired lease looks alive.
pub(super) fn remaining(row: &Map<String, Value>) -> String {
    let expired = flag(row, "expired");
    let seconds = row.get("seconds_remaining").and_then(Value::as_i64);
    match (expired, seconds) {
        (true, Some(value)) => format!("expired {} ago", age(-value)),
        (true, None) => "expired (undatable record)".to_string(),
        (false, Some(value)) => format!("{} left", age(value)),
        (false, None) => "lifetime unknown".to_string(),
    }
}

/// One spelling of a span, the registry's own.
fn age(seconds: i64) -> String {
    chrono::TimeDelta::try_seconds(seconds).map_or_else(
        || "an unreadable span".to_string(),
        crate::cli::registry::human_age,
    )
}
