//! Walking the same dotted path `pull --path` reads, but mutably.
//!
//! Split out of `commands/set.rs`, which had grown past the module line cap;
//! the command itself and its receipt stay there.

use serde_json::Value;

use crate::cli::registry::commands::pull::{kind, select};
use crate::cli::CmdError;

pub(super) const PATH_SEPARATOR: char = '.';
const NAME_FIELD: &str = "name";

/// The value a dotted path names, mutably, creating the last key when the
/// object that would hold it is already there.
///
/// Only the last one: a typo in the middle of a path is still refused with
/// the keys that exist, because inventing `targets.<a misspelt host>` would
/// write a host nothing reads. But a field a new release added and no
/// document carries yet — `…consumers.<consumer>.grants`, for instance —
/// has to be writable by the command the documentation names, or the
/// declaration it describes can never be made at all.
pub(super) fn leaf<'a>(document: &'a mut Value, path: &str) -> Result<&'a mut Value, CmdError> {
    let segments: Vec<&str> = path
        .split(PATH_SEPARATOR)
        .filter(|part| !part.is_empty())
        .collect();
    // A path with no segments names the whole document.
    let Some((last, parents)) = segments.split_last() else {
        return Ok(document);
    };
    let mut value = document;
    let mut walked = String::new();
    for segment in parents {
        value = step(value, segment, &shown(&walked))?;
        if !walked.is_empty() {
            walked.push(PATH_SEPARATOR);
        }
        walked.push_str(segment);
    }
    if let Value::Object(fields) = value {
        if !fields.contains_key(*last) {
            fields.insert(last.to_string(), Value::Null);
        }
    }
    step(value, last, &shown(&walked))
}

/// The path walked so far, as a refusal names it.
fn shown(walked: &str) -> String {
    if walked.is_empty() {
        "<root>".to_string()
    } else {
        walked.to_string()
    }
}

/// The value that would hold this path's last segment, when the document
/// carries it. Used to tell a new field from a mistyped path.
pub(super) fn holder_of<'a>(document: &'a Value, path: &str) -> Option<&'a Value> {
    let segments: Vec<&str> = path
        .split(PATH_SEPARATOR)
        .filter(|part| !part.is_empty())
        .collect();
    let (_, parent) = segments.split_last()?;
    if parent.is_empty() {
        return Some(document);
    }
    // A parent the document does not carry holds nothing: the path is new or mistyped.
    match select(document, &parent.join(&PATH_SEPARATOR.to_string())) {
        Ok(holder) => Some(holder),
        Err(_not_carried) => None,
    }
}

/// One step of the same dotted path `pull --path` reads, mutably. The
/// refusals are the reader's, so a caller that can read a field can write it
/// with the identical path.
fn step<'a>(value: &'a mut Value, segment: &str, walked: &str) -> Result<&'a mut Value, CmdError> {
    match value {
        Value::Object(fields) => {
            if !fields.contains_key(segment) {
                let mut keys: Vec<&str> = fields.keys().map(String::as_str).collect();
                keys.sort_unstable();
                return Err(CmdError::click(format!(
                    "registry has no `{segment}` under `{walked}`; keys there: {}",
                    keys.join(", ")
                ))
                .stating(crate::primitives::failure::FailureCode::NotFound));
            }
            match fields.get_mut(segment) {
                Some(found) => Ok(found),
                None => unreachable!("the key was just found"),
            }
        }
        Value::Array(items) => {
            if let Ok(index) = segment.parse::<usize>() {
                let length = items.len();
                return items.get_mut(index).ok_or_else(|| {
                    CmdError::click(format!(
                        "registry array `{walked}` has {length} element(s), no index {index}"
                    ))
                    .stating(crate::primitives::failure::FailureCode::NotFound)
                });
            }
            let names: Vec<String> = items
                .iter()
                .filter_map(|item| item.get(NAME_FIELD).and_then(Value::as_str))
                .map(str::to_string)
                .collect();
            items
                .iter_mut()
                .find(|item| item.get(NAME_FIELD).and_then(Value::as_str) == Some(segment))
                .ok_or_else(|| {
                    CmdError::click(format!(
                        "registry array `{walked}` has no element named `{segment}`; names there: {}",
                        names.join(", ")
                    ))
                    .stating(crate::primitives::failure::FailureCode::NotFound)
                })
        }
        // A path that walks into a string, number or boolean names nothing
        // the registry holds: the same answer as a missing key.
        other => Err(CmdError::click(format!(
            "registry value at `{walked}` is a {}, which has no `{segment}` inside",
            kind(other)
        ))
        .stating(crate::primitives::failure::FailureCode::NotFound)),
    }
}
