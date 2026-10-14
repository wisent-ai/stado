//! What a pass reads from the canonical registry: this host's target, for its
//! name and its Weles recordings directory, and the release versions the
//! fleet declares. Nothing about cleanup itself is declared there.

pub(crate) mod roots;

use serde_json::{Map, Value};

use crate::providers::local::disk_cleanup::janitor::state::error::JanitorError;
use crate::targets::{self, ComputeTarget};

// ---------------------------------------------------------------------------
// canonical policy resolution
// ---------------------------------------------------------------------------

/// Fetch the canonical registry through this process's configured primary;
/// destructive checks never use fallback/cache. Generation-pinned via the
/// store's versioned read (reload + pinned download, with the same 412-retry
/// the Python SDK path relies on).
///
/// An unreadable registry never authorizes deleting a release version: the
/// release store keeps everything when the fleet's declarations cannot be
/// read. A client configured with the Stado object adapter must read that
/// authority. Its separately configured backup is not a substitute: it may be
/// intentionally distinct, stale, or unable to represent the primary
/// namespace at all.
///
/// DEVIATION from Python, matching `targets::download_registry_blob`: the
/// object is resolved by [`targets::RegistryStore`] instead of a hardcoded GCS
/// bucket.
pub(crate) async fn fetch_canonical_registry() -> Result<Value, JanitorError> {
    let store = targets::RegistryStore::open().await?;
    let text = store
        .read_versioned()
        .await?
        .ok_or_else(|| JanitorError::os("canonical registry generation unavailable"))?;
    let value: Value = serde_json::from_str(&text.content)?;
    if !value.is_object() {
        return Err(JanitorError::value("canonical registry is not an object"));
    }
    Ok(value)
}

/// The identity set of one raw registry target (Python `_identities`).
fn raw_identities(target: &Map<String, Value>) -> Vec<String> {
    let mut values = vec![targets::normalize_hostname(
        target.get("name").and_then(Value::as_str).unwrap_or(""),
    )];
    if let Some(hostnames) = target.get("hostnames").and_then(Value::as_array) {
        for value in hostnames {
            if let Some(value) = value.as_str() {
                values.push(targets::normalize_hostname(value));
            }
        }
    }
    if let Some(ssh) = target.get("ssh").and_then(Value::as_str) {
        if !ssh.is_empty() {
            values.push(targets::ssh_hostname(ssh));
        }
    }
    values
}

/// This host's registry target, matched by its identities.
pub fn resolve_target(data: &Value, hostname: &str) -> Result<ComputeTarget, JanitorError> {
    let identity = targets::normalize_hostname(hostname);
    let targets_arr = data
        .get("targets")
        .and_then(Value::as_array)
        .ok_or_else(|| JanitorError::value("canonical registry has no targets"))?;
    let matches: Vec<&Map<String, Value>> = targets_arr
        .iter()
        .filter_map(Value::as_object)
        .filter(|raw| raw_identities(raw).contains(&identity))
        .collect();
    if matches.len() != 1 {
        return Err(JanitorError::lookup(
            "canonical host identity did not match uniquely",
        ));
    }
    serde_json::from_value(Value::Object(matches[0].clone())).map_err(|error| {
        JanitorError::lookup(&format!("registry target could not be parsed: {error}"))
    })
}
