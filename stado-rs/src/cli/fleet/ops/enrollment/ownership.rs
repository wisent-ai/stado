//! The ownership a registry target declares, written into a registry
//! document (`targets[].ownership`).

use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::targets::{HostOwnership, OWNERSHIP_KEY};

/// `document` with `name`'s ownership set to `ownership`.
pub(super) fn declare_ownership(
    document: &Value,
    name: &str,
    ownership: HostOwnership,
) -> Result<Value, CmdError> {
    let mut next = document.clone();
    let entry = next["targets"]
        .as_array_mut()
        .and_then(|targets| targets.iter_mut().find(|entry| entry["name"] == name))
        .and_then(Value::as_object_mut)
        .ok_or_else(|| CmdError::missing(format!("target '{name}' not found in registry")))?;
    entry.insert(OWNERSHIP_KEY.to_string(), json!(ownership.word()));
    Ok(next)
}
