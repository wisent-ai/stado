//! The one migration the revisit block takes on a write: a label its own
//! product retired when the product moved into one unit is that product's one
//! unit now.

use serde_json::Value;

use super::REVISIT_POLICY_KEY;

/// `document` with every revisit label a product's own catalog entry retired
/// replaced by that product's one unit, or `None` when nothing is renamed.
///
/// Only a rename is rewritten: a label retired by ANOTHER product's process is
/// a different unit, so authorising its replacement would restart something
/// nobody named, and it stays for the contract to refuse. The rewritten block
/// is used only when it then validates; a block that is still invalid for
/// another reason is left exactly as it was, so a write that would have
/// proceeded past the fault still proceeds and still reports it.
pub(crate) fn with_renamed_units(document: &Value) -> Result<Option<Value>, String> {
    let Some(targets) = document
        .get(REVISIT_POLICY_KEY)
        .and_then(|block| block.get("targets"))
        .and_then(Value::as_object)
    else {
        return Ok(None);
    };
    let mut migrated_targets = targets.clone();
    let mut renamed = false;
    for target in migrated_targets.values_mut() {
        let Some(products) = target.get_mut("products").and_then(Value::as_object_mut) else {
            continue;
        };
        for (product, units) in products.iter_mut() {
            let Some(labels) = units.as_array() else {
                continue;
            };
            let mut kept: Vec<Value> = Vec::with_capacity(labels.len());
            for label in labels {
                let current = match label.as_str() {
                    Some(unit) => match crate::deploy::service_catalog::retired_by(unit)? {
                        Some(replacement) if replacement.name == *product => {
                            renamed = true;
                            Value::String(replacement.unit.unwrap_or(replacement.name))
                        }
                        _ => label.clone(),
                    },
                    None => label.clone(),
                };
                if !kept.contains(&current) {
                    kept.push(current);
                }
            }
            *units = Value::Array(kept);
        }
    }
    if !renamed {
        return Ok(None);
    }
    let mut migrated = document.clone();
    migrated[REVISIT_POLICY_KEY]["targets"] = Value::Object(migrated_targets);
    Ok(super::contract::validate_registry_contract(&migrated)
        .is_ok()
        .then_some(migrated))
}
