//! Recursive MIME body collection and Gmail base64 payload decoding.

use base64::Engine;
use serde_json::Value;

pub(super) fn extract_body(payload: &Value, wanted_mime: &str) -> String {
    let mut chunks = Vec::new();
    collect_body_parts(payload, wanted_mime, &mut chunks);
    chunks.join("\n")
}

fn collect_body_parts(part: &Value, wanted_mime: &str, chunks: &mut Vec<String>) {
    let mime = part
        .get("mimeType")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if mime.eq_ignore_ascii_case(wanted_mime) {
        if let Some(data) = part.pointer("/body/data").and_then(Value::as_str) {
            if let Some(text) = decode_gmail_body(data) {
                chunks.push(text);
            }
        }
    }
    for child in part
        .get("parts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        collect_body_parts(child, wanted_mime, chunks);
    }
}

fn decode_gmail_body(data: &str) -> Option<String> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(data)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(data))
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}
