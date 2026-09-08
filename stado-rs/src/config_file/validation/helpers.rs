//! The checks more than one section needs: the placeholder walk over the whole
//! document, the catalog lookup that turns a declared name into a variant, the
//! required-key sweep over that variant's fields, and the storage sections
//! whose keys must all be keys Stado reads.

use serde_json::{Map, Value};

use crate::config_file::readers::{binding_in, py_truthy};

pub(super) fn unresolved_placeholders(value: &Value, path: &str, problems: &mut Vec<String>) {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                if key.starts_with('_') {
                    continue;
                }
                let child = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                unresolved_placeholders(value, &child, problems);
            }
        }
        Value::Array(items) => {
            for (index, value) in items.iter().enumerate() {
                unresolved_placeholders(value, &format!("{path}[{index}]"), problems);
            }
        }
        Value::String(value) if value.contains('<') && value.contains('>') => {
            problems.push(format!(
                "{path} contains unresolved placeholder {value:?}; replace it before deployment"
            ));
        }
        _ => {}
    }
}

pub(super) fn catalog_variant(
    kind: crate::capabilities::RuntimeFacet,
    value: Option<&Value>,
    label: &str,
    problems: &mut Vec<String>,
) -> Option<&'static crate::capabilities::CapabilityVariant> {
    let value = value.filter(|value| !value.is_null())?;
    let variant = value
        .as_str()
        .and_then(|name| crate::capabilities::configurable_variant(kind, name));
    if variant.is_none() {
        let choices = crate::capabilities::configurable_ids(kind)
            .collect::<Vec<_>>()
            .join("|");
        problems.push(format!("{label} must be {choices}, got {value:?}"));
    }
    variant
}

pub(super) fn validate_variant_config(
    root: &Map<String, Value>,
    variant: &crate::capabilities::CapabilityVariant,
    backup: bool,
    problems: &mut Vec<String>,
) {
    for field in variant.config {
        let required = if backup {
            field.backup_required
        } else {
            field.required
        };
        let path = if backup {
            field.backup_path
        } else {
            Some(field.path)
        };
        if !required {
            continue;
        }
        let configured = binding_in(root, path).is_some_and(py_truthy);
        let alternate = binding_in(root, (!backup).then_some(field.alternate_path).flatten())
            .is_some_and(py_truthy);
        if !configured && !alternate {
            problems.push(format!(
                "{}.backend={} needs {}",
                if backup { "storage.backup" } else { "storage" },
                variant.id,
                path.unwrap_or(field.path)
            ));
        }
    }
}

/// A key under a storage adapter must be one Stado actually reads.
///
/// `storage.stado.ca_file` sat in the deployed configuration for as long as the
/// fleet published its object API over TLS, and no code path ever read it. It
/// validated clean, `doctor` was satisfied, and the only storage URL that still
/// worked was a loopback one — so every host quietly addressed its own store
/// while both reported the same shared backend, and two machines held different
/// registries without either noticing. An unknown key is not a harmless extra:
/// it is a setting an operator believes is in effect.
///
/// The catalog is authoritative here in a way it is not for the rest of the
/// document. A storage adapter's `ConfigField` list names every key its backend
/// consumes, so anything else under that section is unread by construction, and
/// saying so at validation time is the difference between a typo and a month of
/// silent divergence. Sections whose adapter the catalog does not know are left
/// alone: this reports keys that cannot be read, never adapters it cannot judge.
pub(super) fn unread_storage_keys(root: &Map<String, Value>, problems: &mut Vec<String>) {
    let Some(storage) = root.get("storage").and_then(Value::as_object) else {
        return;
    };
    for (adapter, section) in storage {
        let Some(section) = section.as_object() else {
            continue;
        };
        let Some(variant) =
            crate::capabilities::variant(crate::capabilities::RuntimeFacet::Storage, adapter)
        else {
            continue;
        };
        let mut known = std::collections::BTreeSet::new();
        for field in variant.config {
            for path in [Some(field.path), field.alternate_path, field.backup_path]
                .into_iter()
                .flatten()
            {
                known.insert(path);
            }
        }
        for key in section.keys() {
            // Operators annotate these documents heavily, and a comment is not a
            // claim about behaviour.
            if key.starts_with('_') {
                continue;
            }
            let path = format!("storage.{adapter}.{key}");
            if !known.contains(path.as_str()) {
                problems.push(format!(
                    "{path} is not a key Stado reads; the {adapter:?} backend consumes only [{}]",
                    known.iter().copied().collect::<Vec<_>>().join(", ")
                ));
            }
        }
    }
}
