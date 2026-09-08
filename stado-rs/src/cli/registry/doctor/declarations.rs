//! Declarations no consumer reads: the derived half from `ComputeTarget`'s
//! unmodelled keys, and the catalogued half from `capabilities`.

use serde_json::Value;

use crate::capabilities::{Consumer, DeclarationSurface, DeclaredField, SiblingCondition};
use crate::cli::registry::doctor::findings::Finding;
use crate::targets::ComputeTarget;

/// The value at a dotted path, or `None` when any segment is absent.
fn value_at<'a>(root: &'a Value, dotted: &str) -> Option<&'a Value> {
    dotted
        .split('.')
        .try_fold(root, |value, key| value.get(key))
}

/// One line of JSON for a declared value, so a finding shows what was written
/// rather than only where.
fn rendered(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
}

/// Why a declared value never reaches behaviour, or `None` when it does.
///
/// The catalog decides, in one place, for both surfaces: a fleet reader whose
/// reachability condition holds is read, and everything else is a declaration an
/// operator wrote for nobody.
fn unread_reason(field: &DeclaredField, sibling: Option<String>) -> Option<String> {
    match field.consumer {
        Consumer::Fleet(reader) => {
            let condition = field.reached_when?;
            let observed = sibling.unwrap_or_else(|| "(absent)".to_string());
            if observed.starts_with(condition.value_prefix) {
                return None;
            }
            Some(format!(
                "its only reader {reader} runs when {} starts with {:?}, and that key is {observed}",
                condition.path, condition.value_prefix
            ))
        }
        Consumer::Unread => Some("no code path in this build reads it".to_string()),
    }
}

/// A sibling value rendered for a finding: strings bare, everything else as JSON,
/// so a URL reads as a URL and a number still reads unambiguously.
fn sibling_value(root: &Value, condition: SiblingCondition) -> Option<String> {
    value_at(root, condition.path).map(|value| {
        value
            .as_str()
            .map_or_else(|| rendered(value), str::to_string)
    })
}

/// Registry fields on one target that no consumer reads, from both halves of the
/// rule.
///
/// The derived half needs no catalog: a key [`ComputeTarget`] does not model
/// lands in `extra`, which is by construction the set of keys no typed reader in
/// this build can name, so a field added tomorrow with no reader fails without
/// anybody remembering to declare it. The catalogued half covers what the derived
/// half cannot see — a path inside a block this build does model, where the
/// deserializer accepting a value proves nothing about anyone acting on it.
pub(super) fn unread_declarations(target: &ComputeTarget, entry: &Value) -> Vec<Finding> {
    let mut findings = Vec::new();
    for (key, value) in &target.extra {
        match crate::capabilities::declared_field(DeclarationSurface::RegistryTarget, key) {
            Some(field) => {
                if let Some(reason) = unread_reason(
                    field,
                    field.reached_when.and_then(|c| sibling_value(entry, c)),
                ) {
                    findings.push(Finding::new(
                        "unread-declaration",
                        &target.name,
                        format!(
                            "{} {key} is declared as {} but {reason}",
                            field.surface.label(),
                            rendered(value)
                        ),
                    ));
                }
            }
            None => findings.push(Finding::new(
                "unread-declaration",
                &target.name,
                format!(
                    "registry target key {key} is declared as {} and is neither modelled by \
                     ComputeTarget nor catalogued in capabilities::DECLARED_FIELDS, so no reader \
                     in this build can consult it",
                    rendered(value)
                ),
            )),
        }
    }
    // Dotted paths only: a top-level catalogued key that this build does not
    // model was already answered by the loop above, and answering it twice would
    // report one defect as two.
    for field in crate::capabilities::DECLARED_FIELDS {
        if field.surface != DeclarationSurface::RegistryTarget || !field.path.contains('.') {
            continue;
        }
        let Some(value) = value_at(entry, field.path) else {
            continue;
        };
        let Some(reason) = unread_reason(
            field,
            field.reached_when.and_then(|c| sibling_value(entry, c)),
        ) else {
            continue;
        };
        findings.push(Finding::new(
            "unread-declaration",
            &target.name,
            format!(
                "{} {} is declared as {} but {reason}",
                field.surface.label(),
                field.path,
                rendered(value)
            ),
        ));
    }
    findings
}

/// Configuration keys this deployment carries that no reader on it can consult.
///
/// The document is the one this process would honour, so the answer is about the
/// deployment actually running rather than about the schema. Reading it cannot
/// fail here: every caller reaches `doctor` through a storage handle that already
/// loaded and parsed the same file.
pub(super) fn unread_configuration() -> Vec<Finding> {
    let subject = crate::config_file::config_path()
        .ok()
        .flatten()
        .map_or_else(
            || "stado config".to_string(),
            |path| path.display().to_string(),
        );
    let mut findings = Vec::new();
    for field in crate::capabilities::DECLARED_FIELDS {
        if field.surface != DeclarationSurface::Configuration {
            continue;
        }
        let Some(value) = crate::config_file::get(field.path).filter(|value| !value.is_null())
        else {
            continue;
        };
        let sibling = field
            .reached_when
            .and_then(|condition| crate::config_file::get(condition.path))
            .map(|value| {
                value
                    .as_str()
                    .map_or_else(|| rendered(&value), str::to_string)
            });
        if let Some(reason) = unread_reason(field, sibling) {
            findings.push(Finding::new(
                "unread-declaration",
                &subject,
                format!(
                    "{} {} is declared as {} but {reason}",
                    field.surface.label(),
                    field.path,
                    rendered(&value)
                ),
            ));
        }
    }
    findings
}
