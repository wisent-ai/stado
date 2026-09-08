//! Regional quota: the ceiling every placement decision runs into first.

use serde_json::{json, Value};

pub(in crate::providers::gcp::inventory) fn region_quota_detail(
    value: &Value,
) -> (&'static str, Option<usize>, Value) {
    let quotas: Vec<Value> = value
        .get("quotas")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|quota| {
            let metric = quota
                .get("metric")
                .and_then(Value::as_str)
                .unwrap_or("UNKNOWN");
            let usage = quota
                .get("usage")
                .and_then(Value::as_f64)
                .unwrap_or_default();
            let limit = quota
                .get("limit")
                .and_then(Value::as_f64)
                .unwrap_or_default();
            json!({
                "metric": metric,
                "usage": usage,
                "limit": limit,
                "exhausted": limit > f64::default() && usage >= limit,
            })
        })
        .collect();
    let exhausted: Vec<&Value> = quotas
        .iter()
        .filter(|quota| quota.get("exhausted").and_then(Value::as_bool) == Some(true))
        .collect();
    let count = quotas.len();
    (
        if exhausted.is_empty() {
            "ok"
        } else {
            "degraded"
        },
        Some(count),
        json!({
            "region": value.get("name"),
            "status": value.get("status"),
            "exhausted": exhausted,
            "quotas": quotas,
        }),
    )
}
