//! Canonical policy resolution: the only authority that may select a
//! cleaner and its retention.

pub(crate) mod roots;
pub(crate) mod watermarks;

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::providers::local::disk_cleanup::janitor::state::error::JanitorError;
use crate::providers::local::disk_cleanup::janitor::state::report::canonical::canonical_json;
use crate::targets::{self, ComputeTarget, DiskCleanupPolicy};

// ---------------------------------------------------------------------------
// canonical policy resolution
// ---------------------------------------------------------------------------

/// Fetch the canonical registry through this process's configured primary;
/// destructive checks never use fallback/cache. Generation-pinned via the
/// store's versioned read (reload + pinned download, with the same 412-retry
/// the Python SDK path relies on).
///
/// The registry remains fail-closed: malformed, incomplete, or unreadable
/// policy never authorizes deletion. A client configured with the Stado object
/// adapter must read that authority. Its separately configured backup is not a
/// substitute: it may be intentionally distinct, stale, or unable to represent
/// the primary namespace at all.
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

/// Resolve the unique local policy from validated canonical data.
///
/// No package registry fallback is permitted. Any fetch, schema, identity, or
/// typing failure is propagated to the caller, which must fail closed.
///
/// A refusal by [`targets::validate_registry`] is NOT a malformed document
/// and is not journalled as one. The parse already succeeded — `data` is a
/// `Value` — so what this rejects is a well-formed registry declaring
/// something this build does not implement, and it carries
/// [`JanitorError::unsupported`]: the entry reads
/// `policy:NotImplementedError` instead of the `policy:ValueError` a corrupt
/// document produces in [`fetch_canonical_registry`].
///
/// The fourth element is true when the host declared no policy and
/// [`DiskCleanupPolicy::reporting_default`] is in force, so a report can say
/// which of the two an operator is looking at. Python
/// `resolve_canonical_policy`.
pub fn resolve_canonical_policy(
    data: &Value,
    hostname: &str,
) -> Result<(ComputeTarget, DiskCleanupPolicy, String, bool), JanitorError> {
    targets::validate_registry(data).map_err(|exc| JanitorError::unsupported(&exc.to_string()))?;
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
    let raw = matches[0];
    if raw.get("kind").and_then(Value::as_str) != Some("local") {
        return Err(JanitorError::lookup(
            "matched target is not a local host, so it has no local cleanup policy",
        ));
    }
    let target: ComputeTarget = serde_json::from_value(Value::Object(raw.clone()))
        .map_err(|_| JanitorError::lookup("cleanup policy could not be parsed"))?;
    // A declared policy wins. A local host that declares none is measured
    // against `DiskCleanupPolicy::reporting_default` rather than refused:
    // returning an error here meant an undeclared host was never scanned and
    // its report carried a `policy` error instead of a free-space number, so
    // the one host that builds every release filled to 1.8 GiB free with
    // nobody watching. Silence in the registry is "nobody has said", not
    // "nothing to do".
    //
    // The digest still comes from whatever policy is in force, so the state
    // file's fencing is unchanged; `defaulted` is what tells the report, and
    // an operator, that no declaration exists.
    let (policy, canonical, defaulted) = match target.disk_cleanup.clone() {
        Some(policy) => (policy, canonical_json(&raw["disk_cleanup"]), false),
        None => {
            let policy = crate::targets::DiskCleanupPolicy::reporting_default();
            let rendered = serde_json::to_value(&policy)
                .map_err(|_| JanitorError::lookup("default cleanup policy could not be built"))?;
            (policy, canonical_json(&rendered), true)
        }
    };
    let digest = format!("{:x}", Sha256::digest(canonical.as_bytes()));
    Ok((target, policy, digest, defaulted))
}
