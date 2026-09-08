//! Field projection shared by every probe detail: the keys worth keeping, the
//! aggregated-list flattening, and the scalar coercions.

use serde_json::Value;

pub(super) fn compact_plain(value: &Value) -> Value {
    let mut detail = serde_json::Map::new();
    for key in [
        "name",
        "id",
        "state",
        "status",
        "location",
        "storageClass",
        "timeCreated",
        "updated",
        "createTime",
        "updateTime",
        "format",
        "kmsKeyName",
        "numBytes",
        "generation",
        "metageneration",
        "etag",
    ] {
        if let Some(entry) = value.get(key) {
            detail.insert(key.to_string(), entry.clone());
        }
    }
    Value::Object(detail)
}

pub(super) fn aggregated<'a>(value: &'a Value, key: &str) -> Vec<&'a Value> {
    value
        .get("items")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|items| items.values())
        .filter_map(|scope| scope.get(key).and_then(Value::as_array))
        .flatten()
        .collect()
}

pub(super) fn text(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
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
