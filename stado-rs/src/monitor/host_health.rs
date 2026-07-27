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
//! resolved from the GCS registry ONLY via [`targets::load_registry_gcs`] —
//! a fetch failure yields an empty registry (no bundled fallback). Tests
//! inject a downloader serving the bundled document.

use serde_json::{json, Map, Value};

use crate::queue::{JobStorage, StorageError};
use crate::targets::{self, ComputeTarget, RegistryError};

/// Beacon blob prefix (Python `HEALTH_PREFIX`).
pub const HEALTH_PREFIX: &str = "host_health";

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
    #[error(transparent)]
    Registry(#[from] RegistryError),
    #[error(transparent)]
    Storage(#[from] StorageError),
}

/// Python `_beacon_slugs`: for each identity (hostnames, name, requested
/// identity, ssh host) the first dot-label and the full normalized form;
/// empty / "/" containing candidates skipped; deduped preserving order.
fn beacon_slugs(target: &ComputeTarget, requested_identity: &str) -> Vec<String> {
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
    let registry = targets::load_registry_gcs().await;
    let target = match registry.lookup(identity) {
        Some(target) => Some(target),
        None => registry.lookup_self(identity)?,
    };
    let target = target.ok_or_else(|| HostHealthError::UnknownTarget(identity.to_string()))?;
    if target.kind != "local" {
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::queue::LocalBackend;

    /// Serve the bundled registry document through the GCS downloader seam
    /// so target resolution (Python source="gcs") stays hermetic offline.
    async fn with_bundled_registry<F, T>(f: F) -> T
    where
        F: std::future::Future<Output = T>,
    {
        let _guard = crate::testutil::GLOBAL_STATE_LOCK.lock().await;
        targets::set_registry_downloader_for_testing(Some(Arc::new(|| {
            Box::pin(async {
                Ok(Some(
                    std::fs::read_to_string(targets::bundled_registry_path())
                        .expect("bundled registry"),
                ))
            })
                as futures::future::BoxFuture<'static, Result<Option<String>, String>>
        })));
        targets::clear_registry_gcs_cache();
        let out = f.await;
        targets::set_registry_downloader_for_testing(None);
        targets::clear_registry_gcs_cache();
        out
    }

    fn store() -> (tempfile::TempDir, JobStorage) {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_str().expect("utf8 path")).expect("backend");
        let store = JobStorage::with_backend_and_bucket(Arc::new(backend), "local", "test-bucket");
        (dir, store)
    }

    fn beacon_json() -> String {
        json!({
            "host": "control-host",
            "reported_at": "2026-07-01T00:00:00+00:00",
            "disk_pct": 42.0,
            "disk_avail_gb": 512.5,
            "units": {
                "stado-agent": {"state": "active"},
                "stado-timer": {"state": "waiting"},
                "legacy-unit": "enabled",
            },
            "last_log": "all systems nominal",
        })
        .to_string()
    }

    #[tokio::test]
    async fn loads_beacon_for_bundled_local_target() {
        with_bundled_registry(async {
            let (_dir, store) = store();
            store
                .upload_text("host_health/control-host.json", &beacon_json())
                .await
                .expect("upload");

            let report = load_host_health(&store, "control-host")
                .await
                .expect("loads");
            assert_eq!(report.target["name"], "control-host");
            assert_eq!(report.target["kind"], "local");
            assert_eq!(
                report.target["hostnames"],
                json!(["control-host.local"])
            );
            assert_eq!(report.beacon["host"], "control-host");

            assert_eq!(
                report.object["uri"],
                "gs://test-bucket/host_health/control-host.json"
            );
            let generation = report.object["generation"]
                .as_str()
                .expect("generation string");
            assert!(!generation.is_empty());
            assert!(report.object["updated_at"].is_string());
            assert!(report.object["created_at"].is_null());
            assert!(report.object["etag"].is_null());
            assert_eq!(report.object["size_bytes"], json!(beacon_json().len()));

            // to_json round-trips the Python dict shape.
            let as_json = report.to_json();
            assert_eq!(as_json["beacon"]["units"]["stado-agent"]["state"], "active");
        })
        .await;
    }

    #[tokio::test]
    async fn resolves_via_hostname_alias_and_falls_through_slugs() {
        with_bundled_registry(async {
            let (_dir, store) = store();
            // Only the full-hostname slug exists; the first-label slug is tried
            // first and missed.
            store
                .upload_text("host_health/control-host.local.json", &beacon_json())
                .await
                .expect("upload");
            let report = load_host_health(&store, "CHARLESS-MAC-MINI.local")
                .await
                .expect("loads");
            assert_eq!(
                report.object["uri"],
                "gs://test-bucket/host_health/control-host.local.json"
            );
        })
        .await;
    }

    #[tokio::test]
    async fn format_renders_all_lines() {
        with_bundled_registry(async {
            let (_dir, store) = store();
            store
                .upload_text("host_health/control-host.json", &beacon_json())
                .await
                .expect("upload");
            let report = load_host_health(&store, "control-host")
                .await
                .expect("loads");
            let text = format_host_health(&report);

            let generation = report.object["generation"].as_str().expect("generation");
            let expected_prefix = format!(
                "target: control-host\n\
                 host: control-host\n\
                 reported_at: 2026-07-01T00:00:00+00:00\n\
                 object_updated_at: {}\n\
                 object: gs://test-bucket/host_health/control-host.json#{generation}\n\
                 disk: 42.0% used; 512.5 GiB available\n\
                 units:\n  \
                 stado-agent: active\n  \
                 stado-timer: waiting\n  \
                 legacy-unit: enabled\n\
                 last_log:\n\
                 all systems nominal",
                report.object["updated_at"].as_str().expect("updated_at"),
            );
            assert_eq!(text, expected_prefix);
            assert!(text.contains("target:"));
            assert!(text.contains("units:"));
            assert!(text.contains("last_log:"));
        })
        .await;
    }

    #[tokio::test]
    async fn format_handles_missing_keys_and_empty_units() {
        let report = HostHealthReport {
            target: json!({"name": "box"}),
            object: json!({"uri": "gs://b/host_health/box.json", "generation": "7",
                           "updated_at": null}),
            beacon: Map::new(),
        };
        let text = format_host_health(&report);
        assert_eq!(
            text,
            "target: box\nhost: -\nreported_at: -\nobject_updated_at: -\n\
             object: gs://b/host_health/box.json#7\ndisk: -% used; - GiB available\n\
             units:\n  -\nlast_log:\n-"
        );
    }

    #[tokio::test]
    async fn unknown_identity_and_non_local_target_errors() {
        with_bundled_registry(async {
            let (_dir, store) = store();
            let err = load_host_health(&store, "no-such-box")
                .await
                .expect_err("unknown");
            assert_eq!(
                err.to_string(),
                r#"target "no-such-box" is not present in the GCS registry"#
            );

            let err = load_host_health(&store, "gcp-spot-t4")
                .await
                .expect_err("not local");
            assert_eq!(
                err.to_string(),
                r#"target "gcp-spot-t4" is not a local registry host"#
            );
        })
        .await;
    }

    #[tokio::test]
    async fn missing_beacon_lists_checked_paths() {
        with_bundled_registry(async {
            let (_dir, store) = store();
            let err = load_host_health(&store, "operator-host")
                .await
                .expect_err("no beacon");
            let message = err.to_string();
            assert!(
                message.starts_with(r#"no host health beacon for "operator-host"; checked "#),
                "{message}"
            );
            assert!(
                message.contains("host_health/operator-host.json"),
                "{message}"
            );
            assert!(
                message.contains("host_health/operator-host.local.json"),
                "{message}"
            );
            assert!(
                message.contains("host_health/operator-host.json"),
                "{message}"
            );
        })
        .await;
    }

    #[tokio::test]
    async fn invalid_beacon_json_and_non_object_errors() {
        with_bundled_registry(async {
            let (_dir, store) = store();
            store
                .upload_text("host_health/control-host.json", "{not json")
                .await
                .expect("upload");
            let err = load_host_health(&store, "control-host")
                .await
                .expect_err("bad json");
            assert_eq!(
                err.to_string(),
                "host health beacon is not valid JSON: gs://test-bucket/host_health/control-host.json"
            );

            store
                .upload_text("host_health/control-host.json", "[1, 2]")
                .await
                .expect("upload");
            let err = load_host_health(&store, "control-host")
                .await
                .expect_err("not object");
            assert_eq!(
                err.to_string(),
                "host health beacon is not an object: gs://test-bucket/host_health/control-host.json"
            );
        })
        .await;
    }
}
