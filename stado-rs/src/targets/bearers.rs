//! The Skarbiec bearers a host holds (`targets[].bearers`).
//!
//! A bearer file on a host is a credential that host can use for as long as
//! the grant it opens stands. On a shared host, a person's own computer, the
//! operator has to be able to read which of the fleet's credentials sit
//! there without opening the machine. Every Stado command that writes a
//! bearer file onto a host records it here, and the command that removes one
//! takes its record away, so `stado registry host show HOST` answers what the
//! host holds.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The registry key of a target's held bearers.
pub const BEARERS_KEY: &str = "bearers";

/// One bearer file Stado placed on a host: the Skarbiec consumer whose grant
/// it opens and its absolute path there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldBearer {
    pub consumer: String,
    pub file: String,
}

/// The target entry named `host` in `document`, as a mutable object.
fn target_entry<'a>(
    document: &'a mut Value,
    host: &str,
) -> Result<&'a mut serde_json::Map<String, Value>, String> {
    document["targets"]
        .as_array_mut()
        .and_then(|targets| targets.iter_mut().find(|entry| entry["name"] == host))
        .and_then(Value::as_object_mut)
        .ok_or_else(|| format!("target '{host}' not found in registry"))
}

/// The bearers `entry` records, refused by name when the field holds
/// something other than a list of `{consumer, file}`.
fn recorded(entry: &serde_json::Map<String, Value>, host: &str) -> Result<Vec<HeldBearer>, String> {
    match entry.get(BEARERS_KEY) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(value) => serde_json::from_value(value.clone()).map_err(|error| {
            format!(
                "targets[{host}].{BEARERS_KEY} is not a list of {{consumer, file}}: {error}; \
                 found {value}"
            )
        }),
    }
}

/// `document` with `host` recorded as holding `bearer`. A record for the same
/// file is replaced, since one file holds one bearer.
pub fn with_held_bearer(
    document: &Value,
    host: &str,
    bearer: &HeldBearer,
) -> Result<Value, String> {
    let mut next = document.clone();
    let entry = target_entry(&mut next, host)?;
    let mut bearers = recorded(entry, host)?;
    bearers.retain(|held| held.file != bearer.file);
    bearers.push(bearer.clone());
    bearers.sort_by(|left, right| left.file.cmp(&right.file));
    entry.insert(
        BEARERS_KEY.to_string(),
        serde_json::to_value(bearers).map_err(|error| error.to_string())?,
    );
    Ok(next)
}

/// `document` with the record of `file` on `host` taken away; the field
/// itself goes when it records nothing more.
pub fn without_held_bearer(document: &Value, host: &str, file: &str) -> Result<Value, String> {
    let mut next = document.clone();
    let entry = target_entry(&mut next, host)?;
    let mut bearers = recorded(entry, host)?;
    bearers.retain(|held| held.file != file);
    if bearers.is_empty() {
        entry.remove(BEARERS_KEY);
    } else {
        entry.insert(
            BEARERS_KEY.to_string(),
            serde_json::to_value(bearers).map_err(|error| error.to_string())?,
        );
    }
    Ok(next)
}
