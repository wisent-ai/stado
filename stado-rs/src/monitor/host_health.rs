//! Read registry-managed host health beacons through Stado.
//!
//! Port of `stado/monitor/host_health.py` (`load_host_health` +
//! `format_host_health`).
//!
//! DEVIATIONS from Python (deliberate):
//! - Python reports full GCS object metadata (created_at, etag, real
//!   size). The storage layer does not expose those, so `created_at` and
//!   `etag` are null and `size_bytes` is the downloaded content length. The
//!   `generation` comes from `read_text_versioned`, whose version IS the GCS
//!   generation — generation-pinned exactly like Python's
//!   `if_generation_match` download.
//!
//! Registry parity: like Python (`lookup(..., source="gcs")`) targets are
//! resolved from the canonical remote registry ONLY, via
//! [`targets::fetch_registry_remote`] — no bundled fallback. DEVIATION: a
//! fetch failure surfaces as [`HostHealthError::RegistryFetch`] instead of
//! Python's empty registry, so "the store is unreachable" no longer
//! reports as "that target does not exist". Tests inject a downloader
//! serving the bundled document.

use serde_json::{json, Map, Value};

use crate::queue::{JobStorage, StorageError};
use crate::targets::{self, ComputeTarget, RegistryError, RegistryFetchError};

/// Beacon blob prefix (Python `HEALTH_PREFIX`).
pub const HEALTH_PREFIX: &str = "host_health";

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
    if let Some(ssh) = &target.ssh {
        identities.push(targets::ssh_hostname(ssh));
    }

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

    for slug in &candidates {
        let path = format!("{HEALTH_PREFIX}/{slug}.json");
        let Some(versioned) = store.read_text_versioned(&path).await? else {
            continue;
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

        let updated_at = store.backend().updated_at(&path).await?;
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
        return Ok(HostHealthReport {
            target: target_json,
            object,
            beacon,
        });
    }

    let attempted = candidates
        .iter()
        .map(|slug| format!("{HEALTH_PREFIX}/{slug}.json"))
        .collect::<Vec<_>>()
        .join(", ");
    Err(HostHealthError::NoBeacon {
        name: target.name.clone(),
        paths: attempted,
    })
}

// ---------------------------------------------------------------------------
// format_host_health
// ---------------------------------------------------------------------------

/// Python `str(value)`: strings raw, null -> "None", everything else in its
/// JSON spelling (serde_json prints floats Python-style, e.g. "85.0").
fn py_str(value: &Value) -> String {
    match value {
        Value::Null => "None".to_string(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Python truthiness for the `x or '-'` fallback.
fn py_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Python `beacon.get(key) or '-'`.
fn py_or_dash(value: Option<&Value>) -> String {
    match value {
        Some(v) if py_truthy(v) => py_str(v),
        _ => "-".to_string(),
    }
}

/// Python `beacon.get(key, '-')` — the default applies only when the key is
/// absent, not when the value is falsy.
fn py_get_or_dash(value: Option<&Value>) -> String {
    value.map_or_else(|| "-".to_string(), py_str)
}

/// Render the health report for an operator without discarding raw logs
/// (Python `format_host_health`, line-for-line).
pub fn format_host_health(report: &HostHealthReport) -> String {
    let beacon = &report.beacon;
    let metadata = &report.object;
    let get = |key: &str| beacon.get(key);

    let mut lines = vec![
        format!("target: {}", py_or_dash(report.target.get("name"))),
        format!("host: {}", py_or_dash(get("host"))),
        format!("reported_at: {}", py_or_dash(get("reported_at"))),
        format!(
            "object_updated_at: {}",
            py_or_dash(metadata.get("updated_at"))
        ),
        format!(
            "object: {}#{}",
            py_str(metadata.get("uri").unwrap_or(&Value::Null)),
            py_str(metadata.get("generation").unwrap_or(&Value::Null)),
        ),
        format!(
            "disk: {}% used; {} GiB available",
            py_get_or_dash(get("disk_pct")),
            py_get_or_dash(get("disk_avail_gb")),
        ),
        "units:".to_string(),
    ];

    if let Some(units) = get("units")
        .and_then(Value::as_object)
        .filter(|u| !u.is_empty())
    {
        for (name, state) in units {
            if let Some(state_map) = state.as_object() {
                let state_value = state_map.get("state");
                let state_text = match state_value {
                    Some(v) if py_truthy(v) => py_str(v),
                    _ => "unknown".to_string(),
                };
                lines.push(format!("  {name}: {state_text}"));
            } else {
                lines.push(format!("  {name}: {}", py_str(state)));
            }
        }
    } else {
        lines.push("  -".to_string());
    }
    lines.push("last_log:".to_string());
    lines.push(py_or_dash(get("last_log")));
    lines.join("\n")
}
