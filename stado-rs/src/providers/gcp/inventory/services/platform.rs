//! The project footing: whether the project is alive, whether it is billable,
//! and whether the caller and the runtime accounts hold what they need.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::providers::gcp::inventory::probes::requirements::{
    REQUIRED_PERMISSIONS, REQUIRED_RUNTIME_ROLES,
};

pub(in crate::providers::gcp::inventory) fn project_detail(
    value: &Value,
) -> (&'static str, Option<usize>, Value) {
    let state = value
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("UNKNOWN");
    (
        if state == "ACTIVE" { "ok" } else { "failed" },
        None,
        json!({
            "name": value.get("name"),
            "project_id": value.get("projectId"),
            "project_number": value.get("name").and_then(Value::as_str).and_then(|name| name.rsplit('/').next()),
            "lifecycle_state": state,
            "create_time": value.get("createTime"),
        }),
    )
}

pub(in crate::providers::gcp::inventory) fn billing_detail(
    value: &Value,
) -> (&'static str, Option<usize>, Value) {
    let enabled = value
        .get("billingEnabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    (
        if enabled { "ok" } else { "failed" },
        None,
        json!({
            "billing_enabled": enabled,
            "billing_account": value.get("billingAccountName"),
        }),
    )
}

pub(in crate::providers::gcp::inventory) fn permissions_detail(
    value: &Value,
) -> (&'static str, Option<usize>, Value) {
    let granted: BTreeSet<&str> = value
        .get("permissions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let missing: Vec<&str> = REQUIRED_PERMISSIONS
        .iter()
        .copied()
        .filter(|permission| !granted.contains(permission))
        .collect();
    (
        if missing.is_empty() { "ok" } else { "degraded" },
        Some(granted.len()),
        json!({"granted": granted, "missing": missing, "required": REQUIRED_PERMISSIONS}),
    )
}

pub(in crate::providers::gcp::inventory) fn project_iam_policy_detail(
    value: &Value,
) -> (&'static str, Option<usize>, Value) {
    let mut roles_by_account = BTreeMap::<String, BTreeSet<String>>::new();
    for binding in value
        .get("bindings")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(role) = binding.get("role").and_then(Value::as_str) else {
            continue;
        };
        for member in binding
            .get("members")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if member.starts_with("serviceAccount:wisent-compute-sa@")
                || member.starts_with("serviceAccount:stado-sa@")
            {
                roles_by_account
                    .entry(member.to_string())
                    .or_default()
                    .insert(role.to_string());
            }
        }
    }
    let required: BTreeSet<String> = REQUIRED_RUNTIME_ROLES
        .iter()
        .map(|role| (*role).to_string())
        .collect();
    let missing_by_account: BTreeMap<String, BTreeSet<String>> = roles_by_account
        .iter()
        .map(|(account, roles)| {
            (
                account.clone(),
                required.difference(roles).cloned().collect(),
            )
        })
        .collect();
    let complete = missing_by_account.values().any(BTreeSet::is_empty);
    let count = roles_by_account.len();
    (
        if complete { "ok" } else { "degraded" },
        Some(count),
        json!({
            "required_runtime_roles": REQUIRED_RUNTIME_ROLES,
            "roles_by_service_account": roles_by_account,
            "missing_by_service_account": missing_by_account,
            "one_runtime_account_complete": complete,
        }),
    )
}
