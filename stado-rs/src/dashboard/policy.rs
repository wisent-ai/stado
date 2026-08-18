//! Canonical registry policy view and generation-checked mutations.
//!
//! Only the disk-cleanup controls exposed by the dashboard are readable or
//! writable here. The full registry contains routing and SSH data and must not
//! be returned by the operator API.
//!
//! This is the WRITE side of `registry.json` and goes through the store
//! `WC_STORAGE_BACKEND` selects. `targets::fetch_registry_remote` is the
//! READ side and resolves the same [`REGISTRY_BLOB`] through the same
//! store, so both address one object on every backend.

use serde_json::{json, Map, Value};

use crate::queue::{BlobBackend, StorageError};
use crate::targets::{clear_registry_cache, validate_registry, REGISTRY_BLOB};

const ROOT_FIELDS: &[&str] = &["target", "disk_cleanup", "pinned_only", "weles"];
const CLEANUP_FIELDS: &[&str] = &[
    "mode",
    "low_free_gb",
    "target_free_gb",
    "max_items_per_pass",
    "max_bytes_per_pass",
    "cleaners",
];
const CLEANUP_SCALAR_FIELDS: &[&str] = &[
    "mode",
    "low_free_gb",
    "target_free_gb",
    "max_items_per_pass",
    "max_bytes_per_pass",
];
/// The build-cache cleaner has no proof flag: the tag file IS the proof.
/// `root` stays out of the dashboard for the same reason weles's does — a
/// scan root is host shape, not an operator dial.
const BUILD_CACHES_CLEANER_FIELDS: &[&str] = &["min_age_seconds"];
const WELES_CLEANER_FIELDS: &[&str] = &["min_age_seconds", "allow_missing_upload_proof"];

#[derive(Debug, thiserror::Error)]
pub enum PolicyError {
    #[error("registry.json does not exist")]
    MissingRegistry,
    #[error("invalid registry JSON: {0}")]
    InvalidRegistry(String),
    #[error("invalid policy request: {0}")]
    InvalidRequest(String),
    #[error("target '{0}' not found")]
    TargetNotFound(String),
    #[error("registry changed while the policy was being saved; refresh and retry")]
    Conflict,
    #[error(transparent)]
    Storage(#[from] StorageError),
}

impl PolicyError {
    pub fn status(&self) -> u16 {
        let code = match self {
            Self::InvalidRequest(_) => "400",
            Self::MissingRegistry | Self::TargetNotFound(_) => "404",
            Self::Conflict => "409",
            Self::InvalidRegistry(_) | Self::Storage(_) => "500",
        };
        code.parse().expect("static HTTP status is valid")
    }
}

fn generation_value(version: &str) -> Value {
    version
        .parse::<u64>()
        .map_or_else(|_| json!(version), |value| json!(value))
}

fn policy_target(target: &Value) -> Option<Value> {
    let object = target.as_object()?;
    let name = object.get("name")?.as_str()?;
    let mut view = Map::new();
    view.insert("name".into(), json!(name));
    for field in ["disk_cleanup", "pinned_only", "weles"] {
        if let Some(value) = object.get(field) {
            view.insert(field.into(), value.clone());
        }
    }
    Some(Value::Object(view))
}

/// Return the generation plus the policy-safe projection of every target.
pub async fn policy_view(backend: &dyn BlobBackend) -> Result<Value, PolicyError> {
    let versioned = backend
        .download_text_versioned(REGISTRY_BLOB)
        .await?
        .ok_or(PolicyError::MissingRegistry)?;
    let registry: Value = serde_json::from_str(&versioned.content)
        .map_err(|error| PolicyError::InvalidRegistry(error.to_string()))?;
    validate_registry(&registry)
        .map_err(|error| PolicyError::InvalidRegistry(error.to_string()))?;
    let targets = registry
        .get("targets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(policy_target)
        .collect::<Vec<_>>();
    Ok(json!({"generation": generation_value(&versioned.version), "targets": targets}))
}

fn reject_unknown(
    object: &Map<String, Value>,
    allowed: &[&str],
    location: &str,
) -> Result<(), PolicyError> {
    let mut unknown = object
        .keys()
        .filter(|key| !allowed.contains(&key.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    unknown.sort();
    if unknown.is_empty() {
        Ok(())
    } else {
        Err(PolicyError::InvalidRequest(format!(
            "{location} contains unsupported field(s): {}",
            unknown.join(", ")
        )))
    }
}

fn object<'a>(value: &'a Value, location: &str) -> Result<&'a Map<String, Value>, PolicyError> {
    value
        .as_object()
        .ok_or_else(|| PolicyError::InvalidRequest(format!("{location} must be an object")))
}

fn merge_fields(destination: &mut Map<String, Value>, patch: &Map<String, Value>, fields: &[&str]) {
    for field in fields {
        if let Some(value) = patch.get(*field) {
            destination.insert((*field).to_string(), value.clone());
        }
    }
}

fn apply_cleanup_patch(target: &mut Map<String, Value>, patch: &Value) -> Result<(), PolicyError> {
    let patch = object(patch, "disk_cleanup")?;
    reject_unknown(patch, CLEANUP_FIELDS, "disk_cleanup")?;
    let cleanup = target
        .get_mut("disk_cleanup")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| PolicyError::InvalidRequest("target has no disk_cleanup policy".into()))?;
    merge_fields(cleanup, patch, CLEANUP_SCALAR_FIELDS);

    if let Some(cleaners_patch) = patch.get("cleaners") {
        let cleaners_patch = object(cleaners_patch, "disk_cleanup.cleaners")?;
        reject_unknown(
            cleaners_patch,
            &["build_caches", "weles_recordings"],
            "disk_cleanup.cleaners",
        )?;
        for (name, fields) in [
            ("build_caches", BUILD_CACHES_CLEANER_FIELDS),
            ("weles_recordings", WELES_CLEANER_FIELDS),
        ] {
            let Some(cleaner_patch) = cleaners_patch.get(name) else {
                continue;
            };
            let location = format!("disk_cleanup.cleaners.{name}");
            let cleaner_patch = object(cleaner_patch, &location)?;
            reject_unknown(cleaner_patch, fields, &location)?;
            let cleaners = cleanup
                .get_mut("cleaners")
                .and_then(Value::as_object_mut)
                .ok_or_else(|| {
                    PolicyError::InvalidRegistry("disk_cleanup.cleaners must be an object".into())
                })?;
            let cleaner = cleaners
                .entry(name)
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .ok_or_else(|| {
                    PolicyError::InvalidRegistry(format!("{location} must be an object"))
                })?;
            merge_fields(cleaner, cleaner_patch, fields);
        }
    }
    Ok(())
}

fn apply_weles_patch(target: &mut Map<String, Value>, patch: &Value) -> Result<(), PolicyError> {
    let patch = object(patch, "weles")?;
    reject_unknown(patch, &["recordings_dir"], "weles")?;
    let weles = target
        .get_mut("weles")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| PolicyError::InvalidRequest("target has no weles configuration".into()))?;
    merge_fields(weles, patch, &["recordings_dir"]);
    Ok(())
}

/// Merge one whitelisted target policy patch and persist it with backend CAS.
pub async fn update_policy(
    backend: &dyn BlobBackend,
    request: &Value,
) -> Result<Value, PolicyError> {
    let request = object(request, "request")?;
    reject_unknown(request, ROOT_FIELDS, "request")?;
    let target_name = request
        .get("target")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| PolicyError::InvalidRequest("target must be a non-empty string".into()))?;

    let versioned = backend
        .download_text_versioned(REGISTRY_BLOB)
        .await?
        .ok_or(PolicyError::MissingRegistry)?;
    let mut registry: Value = serde_json::from_str(&versioned.content)
        .map_err(|error| PolicyError::InvalidRegistry(error.to_string()))?;
    validate_registry(&registry)
        .map_err(|error| PolicyError::InvalidRegistry(error.to_string()))?;

    let target = registry
        .get_mut("targets")
        .and_then(Value::as_array_mut)
        .and_then(|targets| {
            targets
                .iter_mut()
                .find(|target| target.get("name").and_then(Value::as_str) == Some(target_name))
        })
        .ok_or_else(|| PolicyError::TargetNotFound(target_name.to_string()))?;
    let target = target
        .as_object_mut()
        .ok_or_else(|| PolicyError::InvalidRegistry("target must be an object".into()))?;

    if let Some(cleanup) = request.get("disk_cleanup") {
        apply_cleanup_patch(target, cleanup)?;
    }
    if let Some(pinned_only) = request.get("pinned_only") {
        if !pinned_only.is_boolean() {
            return Err(PolicyError::InvalidRequest(
                "pinned_only must be a boolean".into(),
            ));
        }
        target.insert("pinned_only".into(), pinned_only.clone());
    }
    if let Some(weles) = request.get("weles") {
        apply_weles_patch(target, weles)?;
    }

    validate_registry(&registry).map_err(|error| PolicyError::InvalidRequest(error.to_string()))?;
    let body = serde_json::to_string_pretty(&registry)
        .map_err(|error| PolicyError::InvalidRegistry(error.to_string()))?
        + "\n";
    let new_version = match backend
        .compare_and_swap_text(REGISTRY_BLOB, &versioned.version, &body)
        .await
    {
        Ok(version) => version,
        Err(StorageError::StorageConflict(_)) => return Err(PolicyError::Conflict),
        Err(error) => return Err(PolicyError::Storage(error)),
    };
    clear_registry_cache();
    Ok(json!({"ok": true, "generation": generation_value(&new_version)}))
}
