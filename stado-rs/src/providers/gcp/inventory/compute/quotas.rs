//! Regional quota: the ceiling every placement decision runs into first.

use serde_json::{json, Value};

use crate::providers::gcp::inventory::{Detail, ProbeState};

/// One quota as the region states it. A usage or limit the answer does not
/// state stays null and the quota is listed as unread: read as zero, a quota
/// with no stated limit could never read as exhausted, and one with no
/// stated usage read as unused.
fn quota_detail(quota: &Value) -> Value {
    let usage = quota.get("usage").and_then(Value::as_f64);
    let limit = quota.get("limit").and_then(Value::as_f64);
    let exhausted = match (usage, limit) {
        (Some(usage), Some(limit)) => Some(limit > f64::default() && usage >= limit),
        _ => None,
    };
    json!({
        "metric": quota.get("metric"),
        "usage": usage,
        "limit": limit,
        "exhausted": exhausted,
    })
}

pub(in crate::providers::gcp::inventory) fn region_quota_detail(value: &Value) -> Detail {
    let Some(listed) = value.get("quotas").and_then(Value::as_array) else {
        // A region answer without a quotas list is not a region without
        // quotas: the probe cannot say whether placement has room.
        return (
            ProbeState::Degraded,
            None,
            json!({
                "region": value.get("name"),
                "status": value.get("status"),
                "unread": "the region answer carries no quotas list",
            }),
        );
    };
    let quotas: Vec<Value> = listed.iter().map(quota_detail).collect();
    let exhausted: Vec<&Value> = quotas
        .iter()
        .filter(|quota| quota.get("exhausted").and_then(Value::as_bool) == Some(true))
        .collect();
    let unread: Vec<&Value> = quotas
        .iter()
        .filter(|quota| quota.get("exhausted").is_some_and(Value::is_null))
        .collect();
    let count = quotas.len();
    (
        if exhausted.is_empty() && unread.is_empty() {
            ProbeState::Ok
        } else {
            ProbeState::Degraded
        },
        Some(count),
        json!({
            "region": value.get("name"),
            "status": value.get("status"),
            "exhausted": exhausted,
            "unread": unread,
            "quotas": quotas,
        }),
    )
}
