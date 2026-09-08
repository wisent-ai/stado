//! The additive merge itself: what a candidate document accumulates, and the
//! running summary that becomes the receipt.

use serde_json::{Map, Value};

use super::receipt::{RegistryImportConflict, RegistryImportReceipt, RECEIPT_SCHEMA};
use super::source::named_entries;

#[derive(Default)]
pub(super) struct MergeSummary {
    imported_targets: Vec<String>,
    unchanged_targets: Vec<String>,
    imported_fleets: Vec<String>,
    unchanged_fleets: Vec<String>,
    imported_sections: Vec<String>,
    unchanged_sections: Vec<String>,
    pub(super) conflicts: Vec<RegistryImportConflict>,
}

impl MergeSummary {
    pub(super) fn into_receipt(
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
    pub(super) fn discard_pending_imports(&mut self) {
        self.imported_targets.clear();
        self.imported_fleets.clear();
        self.imported_sections.clear();
    }
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

pub(super) fn merge_documents(
    current: &Value,
    source: &Value,
) -> Result<(Value, MergeSummary), String> {
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

pub(super) fn all_source_imported(source: &Value) -> Result<MergeSummary, String> {
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
