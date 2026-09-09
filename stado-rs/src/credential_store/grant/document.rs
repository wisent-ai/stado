//! Reading the owner vault, finding a consumer's grant inside it, and spelling
//! a capability the way the vault records it.

use std::path::Path;

use serde_json::Value;

use crate::skarbiec::SkarbiecError;

pub(super) fn deployment(message: String) -> SkarbiecError {
    SkarbiecError::Deployment(message)
}

/// `action:item#field`, the spelling `token-mint --capabilities` takes and the
/// vault records.
pub(super) fn encode(capability: &Value) -> String {
    let action = capability
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let item = capability
        .get("item")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match capability.get("field").and_then(Value::as_str) {
        Some(field) => format!("{action}:{item}#{field}"),
        None => format!("{action}:{item}"),
    }
}

pub(super) fn now_seconds() -> i64 {
    chrono::Utc::now().timestamp()
}

pub(super) fn read_vault(vault: &Path) -> Result<Value, SkarbiecError> {
    let body = std::fs::read_to_string(vault)
        .map_err(|error| deployment(format!("cannot read vault {}: {error}", vault.display())))?;
    serde_json::from_str(&body).map_err(|error| {
        deployment(format!(
            "vault {} is not valid JSON: {error}",
            vault.display()
        ))
    })
}

pub(super) fn grant_of<'a>(
    document: &'a Value,
    consumer: &str,
) -> Result<&'a Value, SkarbiecError> {
    document
        .get("tokens")
        .and_then(|tokens| tokens.get(consumer))
        .ok_or_else(|| {
            deployment(format!(
                "no grant for consumer {consumer} in the owner vault; mint it deliberately first"
            ))
        })
}
