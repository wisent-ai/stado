//! Readers of what `stado host link` printed.
//!
//! The command's contract with an operator is one JSON document on stdout and
//! whole sentences in `blockers`; these read that document rather than
//! re-deriving anything from the store, so a case asserts what the operator
//! was told.

use std::process::Output;

use chrono::{DateTime, Utc};
use serde_json::Value;

/// The one document on stdout, or a panic carrying what the command said.
pub fn report(out: &Output) -> Value {
    let text = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(&text).unwrap_or_else(|error| {
        panic!(
            "--json output is not one JSON document ({error}):\nstdout:{text}\nstderr:{}",
            stderr(out)
        )
    })
}

pub fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

pub fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// The instant an RFC 3339 string carries.
pub fn instant(raw: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(raw)
        .unwrap_or_else(|error| panic!("{raw:?} is not RFC 3339: {error}"))
        .with_timezone(&Utc)
}

/// The instant a report field carries.
pub fn field_instant(value: &Value) -> DateTime<Utc> {
    instant(
        value
            .as_str()
            .unwrap_or_else(|| panic!("{value} is not a timestamp")),
    )
}

/// The silence records a report carries, newest first as the command orders
/// them.
pub fn silences(report: &Value) -> &Vec<Value> {
    report["silences"]
        .as_array()
        .unwrap_or_else(|| panic!("the report carries no silences array: {report}"))
}

/// The one silence record a report carries, or a panic naming what it carried.
pub fn only_silence(report: &Value) -> &Value {
    let records = silences(report);
    assert_eq!(
        records.len(),
        1,
        "expected exactly one silence record: {report}"
    );
    &records[0]
}

/// The blockers a report names.
pub fn blockers(report: &Value) -> Vec<String> {
    report["blockers"]
        .as_array()
        .unwrap_or_else(|| panic!("the report names no blockers: {report}"))
        .iter()
        .map(|blocker| blocker.as_str().unwrap_or_default().to_string())
        .collect()
}

/// Whether any blocker carries `needle`.
pub fn blocker_saying(report: &Value, needle: &str) -> bool {
    blockers(report)
        .iter()
        .any(|blocker| blocker.contains(needle))
}
