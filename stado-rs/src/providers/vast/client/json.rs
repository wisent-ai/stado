//! The Python-shaped JSON readers and f-string renderers shared by the
//! machine-id lookup, the offer operations and the auto-list bridge. The
//! two renderers the bridge calls carry the wider visibility that reaches
//! it; the readers stay inside the client.
//!
//! Moved verbatim out of the former single-file `providers/vast`.

use serde_json::Value;

use crate::providers::vast::VastError;

/// The machines `/machines/?owner=me` lists: its `machines` list when that
/// holds any, else its `results` list (an empty `machines` falls through, as
/// Vast.ai has answered both ways). An answer with neither list, or with one
/// that is not a list, is refused showing it: read as no machines, it
/// reported a host Vast had simply answered differently as unregistered.
pub(super) fn machines_of(resp: &Value) -> Result<Vec<&Value>, VastError> {
    match (resp.get("machines"), resp.get("results")) {
        (Some(Value::Array(machines)), _) if !machines.is_empty() => Ok(machines.iter().collect()),
        (_, Some(Value::Array(results))) => Ok(results.iter().collect()),
        (Some(Value::Array(machines)), None) => Ok(machines.iter().collect()),
        _ => Err(VastError::config(format!(
            "Vast.ai /machines/?owner=me answered without a machines or results list: {resp}"
        ))),
    }
}

/// Python `int(m["id"])`: numbers pass, numeric strings parse.
pub(super) fn machine_id_of(machine: &Value) -> Result<i64, VastError> {
    machine
        .get("id")
        .and_then(json_int)
        .ok_or_else(|| VastError::config(format!("Vast.ai machine lacks an int id: {machine}")))
}

/// An integer as the vendor writes it: a JSON number, or a numeric string;
/// anything else names no integer, and the caller's refusal quotes the
/// document it came from.
pub(super) fn json_int(value: &Value) -> Option<i64> {
    match value {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => match s.trim().parse() {
            Ok(n) => Some(n),
            Err(_not_an_integer) => None,
        },
        _ => None,
    }
}

/// The text a Vast.ai machine record carries as `field`: `None` when it
/// names none (absent or null), its text when it is a JSON string, and a
/// refusal naming the record otherwise. Reading every other value as empty
/// text hid a hostname Vast sent in another type behind "no hostname".
pub(super) fn machine_text<'a>(machine: &'a Value, field: &str) -> Result<Option<&'a str>, VastError> {
    match machine.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text)),
        Some(other) => Err(VastError::config(format!(
            "Vast.ai machine record carries {field} as {other}, not text: {machine}"
        ))),
    }
}

/// A value Vast.ai sent, as a log line or refusal shows it: its JSON form,
/// or that the answer carried none.
pub(in crate::providers::vast) fn shown(value: Option<&Value>) -> String {
    match value {
        Some(value) => value.to_string(),
        None => "nothing".to_string(),
    }
}

/// Python str(float): integral floats keep one decimal ("3600.0"). A float
/// is integral when it equals its own truncation.
pub(in crate::providers::vast) fn py_float(value: f64) -> String {
    if value.is_finite() && value == value.trunc() {
        format!("{value:.1}")
    } else {
        format!("{value}")
    }
}
