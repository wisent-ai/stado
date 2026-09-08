//! The managed services that feed the coordinator: the scheduler that pokes
//! it, the plain named lists, the service accounts it runs as, and the builds
//! that produced its image.

use serde_json::{json, Value};

pub(in crate::providers::gcp::inventory) fn scheduler_detail(
    value: &Value,
) -> (&'static str, Option<usize>, Value) {
    let state = value
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("UNKNOWN");
    (
        if state == "ENABLED" { "ok" } else { "degraded" },
        Some(true as usize),
        json!({
            "name": value.get("name"),
            "state": state,
            "schedule": value.get("schedule"),
            "time_zone": value.get("timeZone"),
            "http_target": {
                "uri": value.pointer("/httpTarget/uri"),
                "method": value.pointer("/httpTarget/httpMethod"),
                "oidc_service_account": value.pointer("/httpTarget/oidcToken/serviceAccountEmail"),
                "oidc_audience": value.pointer("/httpTarget/oidcToken/audience"),
                "oauth_service_account": value.pointer("/httpTarget/oauthToken/serviceAccountEmail"),
                "oauth_scope": value.pointer("/httpTarget/oauthToken/scope"),
            },
            "last_attempt_time": value.get("lastAttemptTime"),
            "status": value.get("status"),
        }),
    )
}

pub(in crate::providers::gcp::inventory) fn named_list_detail(
    value: &Value,
    key: &str,
) -> (&'static str, Option<usize>, Value) {
    let names: Vec<&str> = value
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("name").and_then(Value::as_str))
        .collect();
    ("ok", Some(names.len()), json!({"names": names}))
}

pub(in crate::providers::gcp::inventory) fn service_accounts_detail(
    value: &Value,
) -> (&'static str, Option<usize>, Value) {
    let accounts: Vec<Value> = value
        .get("accounts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|account| {
            json!({
                "email": account.get("email"),
                "disabled": account.get("disabled"),
                "display_name": account.get("displayName"),
            })
        })
        .collect();
    let required_present = accounts.iter().any(|account| {
        account
            .get("email")
            .and_then(Value::as_str)
            .is_some_and(|email| email.starts_with("wisent-compute-sa@"))
            && account.get("disabled").and_then(Value::as_bool) != Some(true)
    });
    let count = accounts.len();
    (
        if required_present { "ok" } else { "degraded" },
        Some(count),
        json!({"required_service_account_present_and_enabled": required_present, "accounts": accounts}),
    )
}

pub(in crate::providers::gcp::inventory) fn builds_detail(
    value: &Value,
) -> (&'static str, Option<usize>, Value) {
    let builds: Vec<Value> = value
        .get("builds")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|build| {
            json!({
                "id": build.get("id"),
                "status": build.get("status"),
                "create_time": build.get("createTime"),
                "finish_time": build.get("finishTime"),
                "images": build.get("images"),
                "log_url": build.get("logUrl"),
            })
        })
        .collect();
    let latest_failed = builds
        .first()
        .and_then(|build| build.get("status"))
        .and_then(Value::as_str)
        .is_some_and(|status| {
            matches!(
                status,
                "FAILURE" | "INTERNAL_ERROR" | "TIMEOUT" | "CANCELLED" | "EXPIRED"
            )
        });
    let count = builds.len();
    (
        if latest_failed { "degraded" } else { "ok" },
        Some(count),
        json!({"builds": builds}),
    )
}
