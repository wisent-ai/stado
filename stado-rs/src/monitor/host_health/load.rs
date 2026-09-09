//! The reader: where a beacon lives, what one resolves to, and the two
//! documents [`load_host_health`] hands back — the report and the failure.

use serde_json::{json, Map, Value};

use super::HEALTH_PREFIX;
use crate::queue::{JobStorage, StorageError};
use crate::targets::{self, ComputeTarget, RegistryError, RegistryFetchError};

/// Store path one beacon is written to, inside this deployment's namespace.
///
/// The dashboard owns the disk store, so a bare `host_health/<host>.json`
/// looked right from inside it and was invisible to every client that reaches
/// the same store through the object API -- which is every remote agent and,
/// once the operator shares the fleet's channel, the operator too. Writer and
/// readers resolve one path here so "no beacon" means absent, not misfiled.
pub fn beacon_object_path(host: &str) -> String {
    let namespace = crate::config::wc_stado_storage_namespace();
    if namespace.trim().is_empty() {
        return format!("{HEALTH_PREFIX}/{host}.json");
    }
    format!("ecosystem/{namespace}/{HEALTH_PREFIX}/{host}.json")
}

/// One local target's beacon plus immutable object metadata (Python's
/// `load_host_health` dict).
#[derive(Debug, Clone, PartialEq)]
pub struct HostHealthReport {
    /// `{"name", "kind", "hostnames"}` of the resolved target.
    pub target: Value,
    /// `{"uri", "generation", "updated_at", "created_at", "size_bytes", "etag"}`.
    pub object: Value,
    /// The parsed beacon document.
    pub beacon: Map<String, Value>,
}

impl HostHealthReport {
    /// The `--json` rendering: the same shape Python's dict returns.
    pub fn to_json(&self) -> Value {
        json!({"target": self.target, "object": self.object, "beacon": self.beacon})
    }
}

/// `load_host_health` failures, mirroring the Python exception sites
/// (`ValueError` / `FileNotFoundError`) with the Python message text.
#[derive(Debug, thiserror::Error)]
pub enum HostHealthError {
    /// Python `ValueError(f"target {identity!r} is not present in the GCS registry")`.
    #[error("target {0:?} is not present in the GCS registry")]
    UnknownTarget(String),
    /// Python `ValueError(f"target {target.name!r} is not a local registry host")`.
    #[error("target {0:?} is not a local registry host")]
    NotLocal(String),
    /// Python `ValueError("host health beacon is not valid JSON: ...")`.
    #[error("host health beacon is not valid JSON: gs://{bucket}/{path}")]
    InvalidJson { bucket: String, path: String },
    /// Python `ValueError("host health beacon is not an object: ...")`.
    #[error("host health beacon is not an object: gs://{bucket}/{path}")]
    NotAnObject { bucket: String, path: String },
    /// Python `FileNotFoundError(f"no host health beacon for {name!r}; checked ...")`.
    #[error("no host health beacon for {name:?}; checked {paths}")]
    NoBeacon { name: String, paths: String },
    /// The canonical registry could not be READ at all — distinct from a
    /// registry that was read and does not carry the target.
    #[error(transparent)]
    RegistryFetch(#[from] RegistryFetchError),
    #[error(transparent)]
    Registry(#[from] RegistryError),
    #[error(transparent)]
    Storage(#[from] StorageError),
}

/// Python `_beacon_slugs`: for each identity (hostnames, name, requested
/// identity, ssh host) the first dot-label and the full normalized form;
/// empty / "/" containing candidates skipped; deduped preserving order.
///
/// Public because a beacon slug is the only link between a registry target
/// and its `host_health/<slug>.json` object: `cli/registry.rs`'s doctor and
/// beacon-age walk the whole prefix and must resolve slugs back to targets
/// with exactly the rule [`load_host_health`] resolves them forward.
pub fn beacon_slugs(target: &ComputeTarget, requested_identity: &str) -> Vec<String> {
    let mut identities: Vec<String> = target.hostnames.clone();
    identities.push(target.name.clone());
    identities.push(requested_identity.to_string());
    identities.extend(
        target
            .ssh_connections()
            .map(|(_, destination)| targets::ssh_hostname(destination)),
    );

    let mut slugs: Vec<String> = Vec::new();
    for value in &identities {
        let normalized = targets::normalize_hostname(value);
        let first_label = normalized.split('.').next().unwrap_or("").to_string();
        for candidate in [first_label, normalized.clone()] {
            if !candidate.is_empty() && !candidate.contains('/') && !slugs.contains(&candidate) {
                slugs.push(candidate);
            }
        }
    }
    slugs
}

/// Return one local target's beacon plus immutable object metadata.
pub async fn load_host_health(
    store: &JobStorage,
    identity: &str,
) -> Result<HostHealthReport, HostHealthError> {
    let registry = targets::fetch_registry_remote().await?;
    let target = match registry.lookup(identity) {
        Some(target) => Some(target),
        None => registry.lookup_self(identity)?,
    };
    let target = target.ok_or_else(|| HostHealthError::UnknownTarget(identity.to_string()))?;
    if !target.is_provider(crate::capabilities::ProviderId::Local) {
        return Err(HostHealthError::NotLocal(target.name.clone()));
    }

    let bucket = store.bucket_name().to_string();
    let candidates = beacon_slugs(target, identity);
    let mut selected = None;

    // A target may retain more than one registry-owned identity after a
    // hostname change. Candidate order is only a lookup preference; it is not
    // a time authority. Read every existing alias and select the newest object
    // observation so an older alias cannot mask a fresher beacon.
    for slug in &candidates {
        let path = format!("{HEALTH_PREFIX}/{slug}.json");
        let Some(versioned) = store.read_text_versioned(&path).await? else {
            continue;
        };
        let updated_at = store.backend().updated_at(&path).await?;
        let reported_at = serde_json::from_str::<Value>(&versioned.content)
            .ok()
            .and_then(|value| {
                value
                    .get("reported_at")
                    .and_then(Value::as_str)
                    .and_then(|stamp| chrono::DateTime::parse_from_rfc3339(stamp).ok())
                    .map(|stamp| stamp.with_timezone(&chrono::Utc))
            });
        let observed_at = updated_at.or(reported_at);
        let replace = selected
            .as_ref()
            .is_none_or(|(_, _, _, selected_at)| observed_at > *selected_at);
        if replace {
            selected = Some((path, versioned, updated_at, observed_at));
        }
    }

    let Some((path, versioned, updated_at, _)) = selected else {
        let attempted = candidates
            .iter()
            .map(|slug| format!("{HEALTH_PREFIX}/{slug}.json"))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(HostHealthError::NoBeacon {
            name: target.name.clone(),
            paths: attempted,
        });
    };
    let beacon: Value =
        serde_json::from_str(&versioned.content).map_err(|_| HostHealthError::InvalidJson {
            bucket: bucket.clone(),
            path: path.clone(),
        })?;
    let Value::Object(beacon) = beacon else {
        return Err(HostHealthError::NotAnObject {
            bucket: bucket.clone(),
            path: path.clone(),
        });
    };

    let object = json!({
        "uri": format!("gs://{bucket}/{path}"),
        "generation": versioned.version,
        "updated_at": updated_at.map(|ts| ts.to_rfc3339()),
        "created_at": Value::Null,
        "size_bytes": versioned.content.len(),
        "etag": Value::Null,
    });
    let target_json = json!({
        "name": target.name,
        "kind": target.kind,
        "hostnames": target.hostnames,
    });
    Ok(HostHealthReport {
        target: target_json,
        object,
        beacon,
    })
}
