//! A credential field whose stored text is somebody's ciphertext, not a value.
//!
//! An item imported from a store that exports encrypted values (a hosting
//! provider's "sensitive" variable, read without its decryption) carries a
//! `{"v":"v2","c":"…"}` envelope, often base64-encoded, where the value
//! should be. Handed on, it fails much later in the consumer with an error
//! about something else entirely (`Invalid supabaseUrl`), so every string
//! read refuses it here, naming the version it found.

use base64::Engine as _;
use serde_json::Value;

use super::SkarbiecError;

/// The envelope version when `raw` is an encrypted envelope, plain JSON or
/// base64 of it: an object with a string `v` and a string `c` and nothing a
/// value would carry instead. This is a recognizer, not a parser: a value
/// that is not base64, or not JSON, or not that object is a plain value and
/// answers `None`; nothing about it is an error to report.
fn envelope_version(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let decoded = if trimmed.starts_with('{') {
        trimmed.as_bytes().to_vec()
    } else {
        match base64::engine::general_purpose::STANDARD.decode(trimmed) {
            Ok(decoded) => decoded,
            Err(_not_standard_base64) => {
                match base64::engine::general_purpose::URL_SAFE.decode(trimmed) {
                    Ok(decoded) => decoded,
                    Err(_not_base64_at_all) => return None,
                }
            }
        }
    };
    let document: Value = match serde_json::from_slice(&decoded) {
        Ok(document) => document,
        Err(_not_json) => return None,
    };
    let object = document.as_object()?;
    let version = object.get("v")?.as_str()?;
    object.get("c")?.as_str()?;
    Some(version.to_string())
}

/// `value` unchanged, or a refusal when what is stored is an envelope.
pub(crate) fn plain(value: Option<String>) -> Result<Option<String>, SkarbiecError> {
    match value.as_deref().and_then(envelope_version) {
        Some(version) => Err(SkarbiecError::StoredEnvelope { version }),
        None => Ok(value),
    }
}
