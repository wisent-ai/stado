use serde_json::Value;

use crate::cli::host::secrets::weles::trust::{
    SPIS_TRUST_ACTION, SPIS_TRUST_FIELDS, SPIS_TRUST_SCHEMA,
};

/// The public document, checked the way its consumers check it, before it is
/// allowed off the host.
pub(super) fn judge_spis_trust(text: &str) -> Result<(), String> {
    let document: Value =
        serde_json::from_str(text).map_err(|_| "the renderer did not emit one JSON document")?;
    let fields = document
        .as_object()
        .ok_or("the rendered receipt trust is not a JSON object")?;
    if fields.len() != SPIS_TRUST_FIELDS.len()
        || !SPIS_TRUST_FIELDS
            .iter()
            .all(|name| fields.contains_key(*name))
    {
        return Err(format!(
            "the rendered receipt trust must carry exactly {}",
            SPIS_TRUST_FIELDS.join(", ")
        ));
    }
    if fields.get("schema").and_then(Value::as_str) != Some(SPIS_TRUST_SCHEMA) {
        return Err(format!(
            "the rendered receipt trust schema is not {SPIS_TRUST_SCHEMA}"
        ));
    }
    if fields.get("allowedAction").and_then(Value::as_str) != Some(SPIS_TRUST_ACTION) {
        return Err(format!(
            "the rendered allowedAction is not {SPIS_TRUST_ACTION}"
        ));
    }
    let organization = fields
        .get("organizationId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let uuid_shaped = organization.len() == 36
        && organization
            .chars()
            .enumerate()
            .all(|(index, character)| match index {
                8 | 13 | 18 | 23 => character == '-',
                _ => character.is_ascii_hexdigit(),
            });
    if !uuid_shaped {
        return Err("the rendered organizationId is not a UUID".to_string());
    }
    if fields
        .get("keySetVersion")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .is_empty()
    {
        return Err("the rendered keySetVersion is empty".to_string());
    }
    let keys = fields
        .get("receiptKeys")
        .and_then(Value::as_object)
        .ok_or("the rendered receiptKeys is not an object")?;
    if keys.is_empty() {
        return Err("the rendered receiptKeys is empty".to_string());
    }
    for (identifier, key) in keys {
        if identifier.trim().is_empty() {
            return Err("a rendered receipt key identifier is empty".to_string());
        }
        // The verifier hands this string straight to Node's Ed25519 `verify`,
        // which takes a PEM. A base64 body or a DER blob would be accepted
        // here and rejected at the first real receipt.
        match key.as_str() {
            Some(text) if text.contains("-----BEGIN PUBLIC KEY-----") => {}
            _ => return Err(format!("receipt key {identifier} is not a PEM public key")),
        }
    }
    // A private half in a document destined for a public repository is the one
    // mistake this command exists to make impossible.
    if text.contains("PRIVATE KEY") {
        return Err("the rendered document carries private key material".to_string());
    }
    Ok(())
}
