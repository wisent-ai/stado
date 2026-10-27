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

/// A text member of the report, or the member named as not reported, so a
/// blank never stands where the host's answer should be.
pub(super) fn text(report: &Map<String, Value>, key: &str) -> String {
    match report.get(key).and_then(Value::as_str) {
        Some(value) => value.to_string(),
        None => format!("({key} not reported)"),
    }
}

/// A count the report carries, or `none` when it carries no such count.
pub(super) fn count(report: &Map<String, Value>, key: &str) -> String {
    match report.get(key).and_then(Value::as_u64) {
        Some(value) => value.to_string(),
        None => "none".to_string(),
    }
}

pub(super) fn names(report: &Map<String, Value>, key: &str) -> Vec<String> {
    // A report without the list names nothing under it.
    match report.get(key).and_then(Value::as_array) {
        Some(entries) => entries
            .iter()
            .filter_map(|entry| entry.as_str().map(str::to_string))
            .collect(),
        None => Vec::new(),
    }
}

pub(super) fn objects<'a>(
    report: &'a Map<String, Value>,
    key: &str,
) -> Vec<&'a Map<String, Value>> {
    // A report without the list has none of its rows.
    match report.get(key).and_then(Value::as_array) {
        Some(entries) => entries.iter().filter_map(Value::as_object).collect(),
        None => Vec::new(),
    }
}

pub(super) fn field(row: &Map<String, Value>, key: &str) -> String {
    match row.get(key) {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Bool(value)) => value.to_string(),
        Some(Value::Number(value)) => value.to_string(),
        _ => String::new(),
    }
}

/// A flag is set only when the row states it as `true`; absent, null or in
/// another shape, it is not set.
pub(super) fn flag(row: &Map<String, Value>, key: &str) -> bool {
    matches!(row.get(key), Some(Value::Bool(true)))
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

/// One spelling of a span, the registry's own. A span wider than the clock
/// can hold is said as such, not shown as a span nobody computed.
fn age(seconds: i64) -> String {
    match chrono::TimeDelta::try_seconds(seconds) {
        Some(span) => crate::cli::registry::human_age(span),
        None => format!("a span of {seconds} seconds, wider than the clock holds"),
    }
}
