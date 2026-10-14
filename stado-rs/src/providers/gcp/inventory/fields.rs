//! Field projection shared by every probe detail: the keys worth keeping, the
//! aggregated-list flattening, and the scalar coercions.

use serde_json::Value;

/// Google's field names, declared in `inventory-fields.json` beside this
/// file: which fields a probe detail keeps out of a resource document, and
/// the envelope an aggregated list answers with.
fn declared() -> &'static serde_json::Value {
    static DECLARED: std::sync::LazyLock<serde_json::Value> = std::sync::LazyLock::new(|| {
        serde_json::from_str(include_str!("inventory-fields.json"))
            .expect("inventory-fields.json beside this file is valid JSON")
    });
    &DECLARED
}

pub(super) fn compact_plain(value: &Value) -> Value {
    let mut detail = serde_json::Map::new();
    let kept = declared()["kept"]
        .as_array()
        .expect("inventory-fields.json declares the kept fields");
    for key in kept.iter().filter_map(Value::as_str) {
        if let Some(entry) = value.get(key) {
            detail.insert(key.to_string(), entry.clone());
        }
    }
    Value::Object(detail)
}

pub(super) fn aggregated<'a>(value: &'a Value, key: &str) -> Vec<&'a Value> {
    let envelope = declared()["aggregated_items"]
        .as_str()
        .expect("inventory-fields.json declares the aggregated envelope");
    value
        .get(envelope)
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|items| items.values())
        .filter_map(|scope| scope.get(key).and_then(Value::as_array))
        .flatten()
        .collect()
}

/// The last path segment of a resource URL field (`zones/us-central1-a` ->
/// `us-central1-a`), or `null` when the resource does not carry the field:
/// read as "", a disk with a zone reported an empty region beside it and a
/// field Google left out read as one it sent empty.
pub(super) fn tail_of(value: Option<&Value>) -> Value {
    match value.and_then(Value::as_str) {
        Some(text) => Value::from(tail(text)),
        None => Value::Null,
    }
}

pub(super) fn tail(value: impl AsRef<str>) -> String {
    let value = value.as_ref();
    value.rsplit('/').next().unwrap_or(value).to_string()
}

pub(super) fn number_u64(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| value.as_str()?.parse().ok())
}

pub(super) fn encode(value: &str) -> String {
    crate::queue::gcs::percent_encode(value)
}
