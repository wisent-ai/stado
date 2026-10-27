//! The transaction's durable JSON, byte for byte as every earlier release
//! wrote it: keys sorted, no whitespace, and every character from DEL upward
//! escaped as `\uXXXX`. An immutable file published by an earlier release is
//! compared against these bytes, so the encoding is part of the file format.

use std::fs;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::fs::{make_private_dirs, parent_of};
use super::Step;

/// The first character the transaction's JSON escapes: DEL, then everything
/// outside ASCII.
const FIRST_ESCAPED: char = '\u{7f}';

pub(super) fn canonical(value: &Value) -> String {
    let mut out = String::new();
    write_value(value, &mut out);
    out
}

fn write_value(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            let mut separator = "";
            for key in keys {
                out.push_str(separator);
                separator = ",";
                write_string(key, out);
                out.push(':');
                write_value(&map[key], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            let mut separator = "";
            for item in items {
                out.push_str(separator);
                separator = ",";
                write_value(item, out);
            }
            out.push(']');
        }
        Value::String(text) => write_string(text, out),
        other => out.push_str(&other.to_string()),
    }
}

fn write_string(text: &str, out: &mut String) {
    let quoted = Value::String(text.to_string()).to_string();
    for character in quoted.chars() {
        if character < FIRST_ESCAPED {
            out.push(character);
            continue;
        }
        for unit in character.to_string().encode_utf16() {
            out.push_str(&format!("\\u{unit:04x}"));
        }
    }
}

/// Seconds since the epoch, as the receipt's timestamps have always been.
pub(super) fn now() -> Step<Value> {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| format!("the system clock is before 1970: {error}"))?;
    Ok(json!(elapsed.as_secs_f64()))
}

/// A recorded boolean, such as the queue fence's `drained` and `resumed`,
/// which its writer always records; anything but a JSON boolean is refused by
/// name.
pub(super) fn flag(value: Option<&Value>, field: &str) -> Step<bool> {
    match value {
        Some(Value::Bool(flag)) => Ok(*flag),
        None | Some(Value::Null) => Err(format!("{field} is not recorded")),
        Some(other) => Err(format!("{field} is {other}, not a boolean")),
    }
}

/// Whether a time is recorded: absent or null is not, epoch seconds (a
/// positive, normal number) is, and the zero a writer starts a field at is
/// not; any other value is refused by name.
pub(super) fn time_recorded(value: Option<&Value>, field: &str) -> Step<bool> {
    match value {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Number(number)) => Ok(number
            .as_f64()
            .is_some_and(|seconds| seconds.is_normal() && seconds.is_sign_positive())),
        Some(other) => Err(format!("{field} is {other}, not epoch seconds")),
    }
}

/// Whether a text is recorded: absent or null is not, a non-empty string is,
/// and an empty string or any other value is refused by name.
pub(super) fn text_recorded(value: Option<&Value>, field: &str) -> Step<bool> {
    match value {
        None | Some(Value::Null) => Ok(false),
        Some(Value::String(text)) if !text.is_empty() => Ok(true),
        Some(other) => Err(format!("{field} is {other}, not a recorded text")),
    }
}

/// Whether a document is recorded: absent or null is not, a JSON object is,
/// and any other value is refused by name.
pub(super) fn document_recorded(value: Option<&Value>, field: &str) -> Step<bool> {
    match value {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Object(_)) => Ok(true),
        Some(other) => Err(format!("{field} is {other}, not a document")),
    }
}

pub(super) fn list<'a>(value: &'a Value, key: &str, label: &str) -> Step<&'a Vec<Value>> {
    value
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{label} has no {key} list"))
}

pub(super) fn read_json(path: &str) -> Result<Value, String> {
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
}

/// The transaction's canonical bytes at `path`, owner-only, through the one
/// way Stado replaces a file it keeps ([`crate::primitives::document::persist`]):
/// staged, flushed, renamed and the directory flushed.
pub(super) fn atomic_json(path: &str, value: &Value) -> Step<()> {
    make_private_dirs(&parent_of(path)?)?;
    let bytes = format!("{}\n", canonical(value));
    crate::primitives::document::persist(std::path::Path::new(path), bytes.as_bytes())
        .map_err(|error| format!("cannot write {path}: {error}"))
}

/// The document at `path` and its reference: path, SHA-256 and length of
/// the exact bytes read.
pub(super) fn immutable_json_file(path: &str, label: &str) -> Step<(Value, Value)> {
    match fs::symlink_metadata(path) {
        Ok(info) if !info.file_type().is_file() => {
            return Err(format!("{label} is not a regular file: {path}"));
        }
        Ok(_) => {}
        Err(error) => return Err(format!("{label} is absent or invalid: {error}")),
    }
    let encoded =
        fs::read(path).map_err(|error| format!("{label} is absent or invalid: {error}"))?;
    let value: Value = serde_json::from_slice(&encoded)
        .map_err(|error| format!("{label} is absent or invalid: {error}"))?;
    let reference = json!({
        "path": path,
        "sha256": hex::encode(Sha256::digest(&encoded)),
        "bytes": encoded.len(),
    });
    Ok((value, reference))
}

pub(super) fn load_immutable_json(
    reference: Option<&Value>,
    path: &str,
    label: &str,
) -> Step<Value> {
    let Some(reference) = reference.filter(|reference| reference.is_object()) else {
        return Err(format!(
            "{label} reference does not name its canonical transaction file"
        ));
    };
    if reference.get("path").and_then(Value::as_str) != Some(path) {
        return Err(format!(
            "{label} reference does not name its canonical transaction file"
        ));
    }
    let (value, observed) = immutable_json_file(path, label)?;
    if &observed != reference {
        return Err(format!("{label} bytes differ from their durable reference"));
    }
    Ok(value)
}

/// Publish `value` at `path` once; a later call with the same value returns
/// the same reference, and with any other value refuses.
pub(super) fn persist_immutable_json(path: &str, value: &Value, label: &str) -> Step<Value> {
    let encoded = format!("{}\n", canonical(value));
    match fs::symlink_metadata(path) {
        Ok(info) => {
            if !info.file_type().is_file() {
                return Err(format!("{label} collides with a non-regular file: {path}"));
            }
            let existing =
                fs::read(path).map_err(|error| format!("cannot inspect {label}: {error}"))?;
            if existing != encoded.as_bytes() {
                return Err(format!("{label} changed after its immutable publication"));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => atomic_json(path, value)?,
        Err(error) => return Err(format!("cannot inspect {label}: {error}")),
    }
    Ok(immutable_json_file(path, label)?.1)
}
