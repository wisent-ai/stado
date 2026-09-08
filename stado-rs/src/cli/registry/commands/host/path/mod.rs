//! `stado registry host path list|set|remove` — the ordered SSH connection
//! paths one registry target declares, and the index every one of them
//! resolves the target through.

pub(in crate::cli::registry) mod list;
pub(in crate::cli::registry) mod remove;
pub(in crate::cli::registry) mod set;

use serde_json::Value;

use crate::cli::CmdError;
use crate::targets;

fn registry_host_index(document: &Value, host: &str) -> Result<(usize, String), CmdError> {
    let name = targets::normalize_hostname(host);
    if name.is_empty() {
        return Err(CmdError::click("HOST must not be empty"));
    }
    let entries = document
        .get("targets")
        .and_then(Value::as_array)
        .ok_or_else(|| CmdError::click("registry.targets: must be an array"))?;
    let index = entries
        .iter()
        .position(|entry| {
            entry
                .get("name")
                .and_then(Value::as_str)
                .map(targets::normalize_hostname)
                .as_deref()
                == Some(name.as_str())
        })
        .ok_or_else(|| CmdError::click(format!("registry target {name:?} not found")))?;
    Ok((index, name))
}
