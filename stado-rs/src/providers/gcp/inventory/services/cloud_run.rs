//! The Cloud Run coordinator: its readiness, who may invoke it, and the
//! revisions behind it.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

pub(in crate::providers::gcp::inventory) fn cloud_run_service_detail(
    value: &Value,
) -> (&'static str, Option<usize>, Value) {
    let terminal = value
        .pointer("/terminalCondition/state")
        .and_then(Value::as_str)
        .unwrap_or("UNKNOWN");
    let latest_ready = value.get("latestReadyRevision").and_then(Value::as_str);
    let latest_created = value.get("latestCreatedRevision").and_then(Value::as_str);
    let ready = terminal == "CONDITION_SUCCEEDED"
        && latest_ready.is_some()
        && latest_ready == latest_created;
    let mut environment = BTreeMap::new();
    let catalog_environment = [
        crate::capabilities::config_env(
            crate::capabilities::RuntimeFacet::Compute,
            crate::capabilities::ProviderId::Gcp.as_str(),
            "project",
        )
        .expect("GCP project binding is missing from the capability catalog"),
        crate::capabilities::config_env(
            crate::capabilities::RuntimeFacet::Storage,
            crate::capabilities::StorageAdapter::Gcs.id(),
            "bucket",
        )
        .expect("GCS bucket binding is missing from the capability catalog"),
        crate::capabilities::PROVIDERS_CONFIG.env,
        crate::capabilities::STORAGE_BACKEND_CONFIG.env,
    ];
    for container in value
        .pointer("/template/containers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        for variable in container
            .get("env")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(name) = variable.get("name").and_then(Value::as_str) else {
                continue;
            };
            if catalog_environment.contains(&name)
                || matches!(
                    name,
                    "GOOGLE_CLOUD_PROJECT"
                        | "WC_ALERTS_TOPIC"
                        | "WC_COORDINATOR_ID"
                        | "STADO_DEPLOYMENT_ID"
                        | "STADO_API_URL"
                        | "STADO_RELEASE_VERSION"
                        | "STADO_RELEASE_PLATFORM"
                )
            {
                environment.insert(name, variable.get("value").and_then(Value::as_str));
            }
        }
    }
    (
        if ready { "ok" } else { "degraded" },
        Some(true as usize),
        json!({
            "name": value.get("name"),
            "uri": value.get("uri"),
            "terminal_condition": value.get("terminalCondition"),
            "latest_ready_revision": latest_ready,
            "latest_created_revision": latest_created,
            "service_account": value.pointer("/template/serviceAccount"),
            "environment": environment,
            "update_time": value.get("updateTime"),
        }),
    )
}

pub(in crate::providers::gcp::inventory) fn cloud_run_iam_policy_detail(
    value: &Value,
) -> (&'static str, Option<usize>, Value) {
    let invokers: BTreeSet<&str> = value
        .get("bindings")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|binding| binding.get("role").and_then(Value::as_str) == Some("roles/run.invoker"))
        .flat_map(|binding| {
            binding
                .get("members")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
        })
        .collect();
    let authenticated_invoker = invokers
        .iter()
        .any(|member| member.starts_with("serviceAccount:"));
    let publicly_invokable = invokers.contains("allUsers");
    let state = if authenticated_invoker && !publicly_invokable {
        "ok"
    } else {
        "degraded"
    };
    (
        state,
        Some(invokers.len()),
        json!({
            "invokers": invokers,
            "authenticated_service_account_invoker": authenticated_invoker,
            "publicly_invokable": publicly_invokable,
            "etag": value.get("etag"),
        }),
    )
}

pub(in crate::providers::gcp::inventory) fn cloud_run_revisions_detail(
    value: &Value,
) -> (&'static str, Option<usize>, Value) {
    let revisions: Vec<Value> = value
        .get("revisions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|revision| {
            json!({
                "name": revision.get("name"),
                "terminal_condition": revision.get("terminalCondition"),
                "service_account": revision.get("serviceAccount"),
                "create_time": revision.get("createTime"),
            })
        })
        .collect();
    let unhealthy = revisions
        .iter()
        .filter(|revision| {
            revision
                .pointer("/terminal_condition/state")
                .and_then(Value::as_str)
                != Some("CONDITION_SUCCEEDED")
        })
        .count();
    let count = revisions.len();
    (
        if unhealthy == usize::default() {
            "ok"
        } else {
            "degraded"
        },
        Some(count),
        json!({"unhealthy": unhealthy, "revisions": revisions}),
    )
}
