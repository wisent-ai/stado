//! Additive, idempotent adoption of an existing Stado registry-v2 document.
//!
//! The importer is the one operation used by the CLI and dashboard API. It
//! validates the complete source before opening the destination, merges named
//! fleet records without replacing anything already declared, validates the
//! complete candidate, and commits it with compare-and-swap. A replay either
//! reports every source record as unchanged or imports only records that are
//! still absent.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

use crate::queue::StorageError;
use crate::targets::{self, RegistryStore};

const RECEIPT_SCHEMA: &str = "stado.registry-import-receipt.v1";
const MAX_COMMIT_ROUNDS: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryImportConflict {
    pub path: String,
    pub reason: String,
}

/// The complete, bounded answer returned by both public surfaces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryImportReceipt {
    pub schema: String,
    pub state: String,
    pub source_sha256: String,
    pub generation: Option<String>,
    pub previous_generation: Option<String>,
    pub imported_targets: Vec<String>,
    pub unchanged_targets: Vec<String>,
    pub imported_fleets: Vec<String>,
    pub unchanged_fleets: Vec<String>,
    pub imported_sections: Vec<String>,
    pub unchanged_sections: Vec<String>,
    pub conflicts: Vec<RegistryImportConflict>,
    pub rejected: Vec<String>,
}

impl RegistryImportReceipt {
    fn empty(source_sha256: String, state: &str) -> Self {
        Self {
            schema: RECEIPT_SCHEMA.to_string(),
            state: state.to_string(),
            source_sha256,
            generation: None,
            previous_generation: None,
            imported_targets: Vec::new(),
            unchanged_targets: Vec::new(),
            imported_fleets: Vec::new(),
            unchanged_fleets: Vec::new(),
            imported_sections: Vec::new(),
            unchanged_sections: Vec::new(),
            conflicts: Vec::new(),
            rejected: Vec::new(),
        }
    }

    pub fn accepted(&self) -> bool {
        matches!(self.state.as_str(), "imported" | "unchanged")
    }

    pub fn changed(&self) -> bool {
        self.state == "imported"
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryImportError {
    #[error("registry import could not open or persist the canonical registry: {0}")]
    Storage(String),
    #[error("canonical registry at generation {generation} is invalid: {reason}")]
    CanonicalInvalid { generation: String, reason: String },
    #[error("registry import verification returned different bytes or generation")]
    Verification,
}

impl From<StorageError> for RegistryImportError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error.to_string())
    }
}

#[derive(Default)]
struct MergeSummary {
    imported_targets: Vec<String>,
    unchanged_targets: Vec<String>,
    imported_fleets: Vec<String>,
    unchanged_fleets: Vec<String>,
    imported_sections: Vec<String>,
    unchanged_sections: Vec<String>,
    conflicts: Vec<RegistryImportConflict>,
}

impl MergeSummary {
    fn into_receipt(
        mut self,
        source_sha256: String,
        state: &str,
        generation: Option<String>,
        previous_generation: Option<String>,
    ) -> RegistryImportReceipt {
        self.imported_targets.sort();
        self.unchanged_targets.sort();
        self.imported_fleets.sort();
        self.unchanged_fleets.sort();
        self.imported_sections.sort();
        self.imported_sections.dedup();
        self.unchanged_sections.sort();
        self.unchanged_sections.dedup();
        self.conflicts
            .sort_by(|left, right| left.path.cmp(&right.path));
        RegistryImportReceipt {
            schema: RECEIPT_SCHEMA.to_string(),
            state: state.to_string(),
            source_sha256,
            generation,
            previous_generation,
            imported_targets: self.imported_targets,
            unchanged_targets: self.unchanged_targets,
            imported_fleets: self.imported_fleets,
            unchanged_fleets: self.unchanged_fleets,
            imported_sections: self.imported_sections,
            unchanged_sections: self.unchanged_sections,
            conflicts: self.conflicts,
            rejected: Vec::new(),
        }
    }
    fn discard_pending_imports(&mut self) {
        self.imported_targets.clear();
        self.imported_fleets.clear();
        self.imported_sections.clear();
    }
}

fn validate_document(document: &Value) -> Result<(), String> {
    targets::validate_registry(document).map_err(|error| error.to_string())?;
    crate::cli::fleet::fleets::parse_fleets(document)?;
    for section in ["targets", "fleets", "coordinators", "placement_profiles"] {
        named_entries(document.get(section), section)?;
    }
    Ok(())
}

fn named_entries<'a>(
    value: Option<&'a Value>,
    section: &str,
) -> Result<Vec<(&'a str, &'a Value)>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let entries = value
        .as_array()
        .ok_or_else(|| format!("registry.{section}: must be an array"))?;
    let mut names = HashSet::with_capacity(entries.len());
    let mut named = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let name = entry
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| {
                format!("registry.{section}[{index}].name: must be a non-empty string")
            })?;
        if !names.insert(name) {
            return Err(format!(
                "registry.{section}[{index}].name: duplicate name {name:?}"
            ));
        }
        named.push((name, entry));
    }
    Ok(named)
}

fn source_rejection(bytes: &[u8], reason: String) -> RegistryImportReceipt {
    let mut receipt =
        RegistryImportReceipt::empty(format!("{:x}", Sha256::digest(bytes)), "rejected");
    receipt.rejected.push(reason);
    receipt
}

/// Decode and validate the complete source without reading or mutating the
/// canonical destination. Rejections therefore cannot leave a partial import.
fn decode_source(bytes: &[u8]) -> Result<Value, String> {
    let document: Value = serde_json::from_slice(bytes)
        .map_err(|error| format!("source is not valid JSON: {error}"))?;
    validate_document(&document).map_err(|error| format!("source registry is invalid: {error}"))?;
    Ok(document)
}

fn merge_named_array(
    candidate: &mut Map<String, Value>,
    source: &Map<String, Value>,
    section: &str,
    summary: &mut MergeSummary,
) -> Result<(), String> {
    let source_entries = named_entries(source.get(section), section)?;
    if !source.contains_key(section) {
        return Ok(());
    }
    let destination_had_section = candidate.contains_key(section);
    let destination = candidate
        .entry(section.to_string())
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(|| format!("canonical registry.{section}: must be an array"))?;

    let mut imported_any = false;
    let mut unchanged_any = false;
    for (name, incoming) in source_entries {
        match destination
            .iter()
            .find(|entry| entry.get("name").and_then(Value::as_str) == Some(name))
        {
            Some(existing) if existing == incoming => {
                unchanged_any = true;
                match section {
                    "targets" => summary.unchanged_targets.push(name.to_string()),
                    "fleets" => summary.unchanged_fleets.push(name.to_string()),
                    _ => {}
                }
            }
            Some(_) => summary.conflicts.push(RegistryImportConflict {
                path: format!("registry.{section}[name={name:?}]"),
                reason: format!(
                    "the canonical registry already declares {section} record {name:?} with different content"
                ),
            }),
            None => {
                destination.push(incoming.clone());
                imported_any = true;
                match section {
                    "targets" => summary.imported_targets.push(name.to_string()),
                    "fleets" => summary.imported_fleets.push(name.to_string()),
                    _ => {}
                }
            }
        }
    }
    if imported_any || !destination_had_section {
        summary.imported_sections.push(section.to_string());
    } else if unchanged_any
        || source
            .get(section)
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty)
    {
        summary.unchanged_sections.push(section.to_string());
    }
    Ok(())
}

/// Add missing object fields recursively. Arrays and scalar values have no
/// general identity in registry-v2, so differing values conflict instead of
/// being guessed, replaced, or silently dropped.
fn merge_value(
    current: &mut Value,
    incoming: &Value,
    path: &str,
    conflicts: &mut Vec<RegistryImportConflict>,
) -> bool {
    if current == incoming {
        return false;
    }
    match (current, incoming) {
        (Value::Object(destination), Value::Object(source)) => {
            let mut changed = false;
            for (key, value) in source {
                let child = format!("{path}.{key}");
                match destination.get_mut(key) {
                    Some(existing) => {
                        changed |= merge_value(existing, value, &child, conflicts);
                    }
                    None => {
                        destination.insert(key.clone(), value.clone());
                        changed = true;
                    }
                }
            }
            changed
        }
        _ => {
            conflicts.push(RegistryImportConflict {
                path: path.to_string(),
                reason: "the canonical registry already carries a different value; import never replaces existing registry state".to_string(),
            });
            false
        }
    }
}

fn merge_documents(current: &Value, source: &Value) -> Result<(Value, MergeSummary), String> {
    let mut candidate = current.clone();
    let destination = candidate
        .as_object_mut()
        .ok_or_else(|| "canonical registry must be an object".to_string())?;
    let source = source
        .as_object()
        .ok_or_else(|| "source registry must be an object".to_string())?;
    let mut summary = MergeSummary::default();

    for section in ["targets", "fleets", "coordinators", "placement_profiles"] {
        merge_named_array(destination, source, section, &mut summary)?;
    }

    for (key, incoming) in source {
        if matches!(
            key.as_str(),
            "targets" | "fleets" | "coordinators" | "placement_profiles"
        ) {
            continue;
        }
        match destination.get_mut(key) {
            Some(existing) => {
                let changed = merge_value(
                    existing,
                    incoming,
                    &format!("registry.{key}"),
                    &mut summary.conflicts,
                );
                if changed {
                    summary.imported_sections.push(key.clone());
                } else if existing == incoming {
                    summary.unchanged_sections.push(key.clone());
                }
            }
            None => {
                destination.insert(key.clone(), incoming.clone());
                summary.imported_sections.push(key.clone());
            }
        }
    }

    Ok((candidate, summary))
}

fn all_source_imported(source: &Value) -> Result<MergeSummary, String> {
    let mut summary = MergeSummary::default();
    for (name, _) in named_entries(source.get("targets"), "targets")? {
        summary.imported_targets.push(name.to_string());
    }
    for (name, _) in named_entries(source.get("fleets"), "fleets")? {
        summary.imported_fleets.push(name.to_string());
    }
    if let Some(root) = source.as_object() {
        summary.imported_sections.extend(
            root.keys()
                .filter(|key| key.as_str() != "schema_version")
                .cloned(),
        );
        if root.contains_key("schema_version") {
            summary
                .unchanged_sections
                .push("schema_version".to_string());
        }
    }
    Ok(summary)
}

async fn verify_write(
    store: &RegistryStore,
    expected_generation: &str,
    expected_content: &str,
) -> Result<(), RegistryImportError> {
    let confirmed = store
        .read_versioned()
        .await?
        .ok_or(RegistryImportError::Verification)?;
    if confirmed.version != expected_generation || confirmed.content != expected_content {
        return Err(RegistryImportError::Verification);
    }
    targets::clear_registry_cache();
    Ok(())
}

/// Import one complete registry-v2 document into the configured canonical
/// registry. Semantic conflicts and invalid inputs are receipts, not partial
/// failures; storage failures are operational errors.
pub async fn import_bytes(bytes: &[u8]) -> Result<RegistryImportReceipt, RegistryImportError> {
    let source = match decode_source(bytes) {
        Ok(source) => source,
        Err(reason) => return Ok(source_rejection(bytes, reason)),
    };
    let source_sha256 = format!("{:x}", Sha256::digest(bytes));
    let store = RegistryStore::open().await?;
    let mut last_generation = None;

    for _ in 0..MAX_COMMIT_ROUNDS {
        let current = store.read_versioned().await?;
        let Some(current) = current else {
            let payload = format!(
                "{}\n",
                serde_json::to_string_pretty(&source).map_err(|error| {
                    RegistryImportError::Storage(format!(
                        "cannot serialize source registry: {error}"
                    ))
                })?
            );
            if !store.create_if_absent(&payload).await? {
                continue;
            }
            let generation = store
                .read_versioned()
                .await?
                .ok_or(RegistryImportError::Verification)?
                .version;
            verify_write(&store, &generation, &payload).await?;
            let summary = all_source_imported(&source).map_err(RegistryImportError::Storage)?;
            return Ok(summary.into_receipt(
                source_sha256,
                "imported",
                Some(generation),
                Some("absent".to_string()),
            ));
        };
        last_generation = Some(current.version.clone());
        let canonical: Value = serde_json::from_str(&current.content).map_err(|error| {
            RegistryImportError::CanonicalInvalid {
                generation: current.version.clone(),
                reason: format!("not valid JSON: {error}"),
            }
        })?;
        validate_document(&canonical).map_err(|reason| RegistryImportError::CanonicalInvalid {
            generation: current.version.clone(),
            reason,
        })?;

        let (candidate, mut summary) =
            merge_documents(&canonical, &source).map_err(RegistryImportError::Storage)?;
        if !summary.conflicts.is_empty() {
            summary.discard_pending_imports();
            return Ok(summary.into_receipt(
                source_sha256,
                "conflict",
                Some(current.version),
                None,
            ));
        }
        if let Err(reason) = validate_document(&candidate) {
            summary.conflicts.push(RegistryImportConflict {
                path: "registry".to_string(),
                reason: format!("combining the two valid registries is invalid: {reason}"),
            });
            summary.discard_pending_imports();
            return Ok(summary.into_receipt(
                source_sha256,
                "conflict",
                Some(current.version),
                None,
            ));
        }
        if candidate == canonical {
            return Ok(summary.into_receipt(
                source_sha256,
                "unchanged",
                Some(current.version),
                None,
            ));
        }

        let payload = format!(
            "{}\n",
            serde_json::to_string_pretty(&candidate).map_err(|error| {
                RegistryImportError::Storage(format!("cannot serialize merged registry: {error}"))
            })?
        );
        match store.compare_and_swap(&current.version, &payload).await {
            Ok(generation) => {
                verify_write(&store, &generation, &payload).await?;
                return Ok(summary.into_receipt(
                    source_sha256,
                    "imported",
                    Some(generation),
                    Some(current.version),
                ));
            }
            Err(StorageError::StorageConflict(_)) => continue,
            Err(error) => return Err(error.into()),
        }
    }

    let mut receipt = RegistryImportReceipt::empty(source_sha256, "conflict");
    receipt.generation = last_generation;
    receipt.conflicts.push(RegistryImportConflict {
        path: "registry".to_string(),
        reason: format!(
            "the canonical registry moved during all {MAX_COMMIT_ROUNDS} conditional import attempts; no import write was accepted"
        ),
    });
    Ok(receipt)
}
