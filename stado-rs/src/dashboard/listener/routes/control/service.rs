//! The managed-service plane: one service's beacon status, restarting it on
//! every declared host, and reporting or applying one host's convergence.

use serde_json::{json, Value};

use crate::deploy::{host_channel, production_runner, service};
use crate::queue::JobStorage;
use crate::targets;

use crate::dashboard::listener::http::strict_url_decode;
use crate::dashboard::listener::{http_status, send_json, Dashboard, Request, Response};

impl Dashboard {
    pub(crate) async fn service_converge(
        &self,
        request: &Request,
        query: &str,
        apply: bool,
    ) -> Response {
        if request.header("transfer-encoding").is_some()
            || request.content_length != usize::default()
            || !request.body.is_empty()
        {
            return invalid_service_request("service converge does not accept a request body");
        }
        let (target, binary) = match service_converge_query(query) {
            Ok(scope) => scope,
            Err(response) => return response,
        };
        match crate::cli::service_converge::converge_result(&target, binary.as_deref(), apply).await
        {
            Ok(result) => send_json(
                http_status("200"),
                &json!({"exit_code": result.exit_code, "report": result.report_json()}),
            ),
            Err(error) => service_failure(
                http_status("503"),
                "SERVICE_CONVERGE_FAILED",
                error.to_string(),
                true,
            ),
        }
    }

    pub(crate) async fn get_service_status(&self, request: &Request, query: &str) -> Response {
        if request.content_length != usize::default() || !request.body.is_empty() {
            return invalid_service_request("service status does not accept a request body");
        }
        let name = match service_name(query) {
            Ok(name) => name,
            Err(response) => return response,
        };
        let store = match service_beacon_store().await {
            Ok(store) => store,
            Err(message) => {
                return service_failure(http_status("503"), "SERVICE_STATUS_FAILED", message, true)
            }
        };
        let rows = match service::find_services(&store, name).await {
            Ok(rows) => rows,
            Err(error) => {
                return service_failure(
                    http_status("503"),
                    "SERVICE_STATUS_FAILED",
                    error.to_string(),
                    true,
                )
            }
        };
        if rows.is_empty() {
            return service_failure(
                http_status("404"),
                "NOT_FOUND",
                format!("no registry-managed service named {name}"),
                false,
            );
        }
        service_success(Value::Array(
            rows.iter().map(service::ServiceStatus::to_json).collect(),
        ))
    }

    pub(crate) async fn post_service_restart(&self, request: &Request, query: &str) -> Response {
        if request.header("transfer-encoding").is_some()
            || request.content_length != usize::default()
            || !request.body.is_empty()
        {
            return invalid_service_request("service restart does not accept a request body");
        }
        let name = match service_name(query) {
            Ok(name) => name,
            Err(response) => return response,
        };
        let services = match declared_services_matching(name).await {
            Ok(services) => services,
            Err(message) => {
                return service_failure(http_status("503"), "SERVICE_RESTART_FAILED", message, true)
            }
        };
        if services.is_empty() {
            return service_failure(
                http_status("404"),
                "NOT_FOUND",
                format!("no registry-managed service named {name}"),
                false,
            );
        }
        let runner = production_runner();
        let mut result = Vec::with_capacity(services.len());
        let mut failures = Vec::new();
        for declared in &services {
            let target = match host_channel::canonical_target(&declared.host).await {
                Ok(target) => target,
                Err(error) => {
                    return service_failure(
                        http_status("503"),
                        "SERVICE_RESTART_FAILED",
                        error.to_string(),
                        true,
                    )
                }
            };
            let report = match service::restart_service(&target, declared, &runner).await {
                Ok(report) => report,
                Err(error) => {
                    return service_failure(
                        http_status("503"),
                        "SERVICE_RESTART_FAILED",
                        error.to_string(),
                        true,
                    )
                }
            };
            if !report.succeeded("restarted") {
                failures.push(format!("{}: {}", declared.host, report.failure()));
            }
            let mut entry = report.to_json();
            entry["host"] = Value::from(declared.host.clone());
            result.push(entry);
        }
        if !failures.is_empty() {
            return service_failure(
                http_status("503"),
                "SERVICE_RESTART_FAILED",
                format!("restart failed on {}", failures.join("; ")),
                true,
            );
        }
        service_success(Value::Array(result))
    }
}

pub(crate) fn service_name(query: &str) -> Result<&str, Response> {
    let invalid = || {
        invalid_service_request(
            "query must contain exactly one lowercase, path-safe name parameter",
        )
    };
    if query.is_empty() || query.contains('&') {
        return Err(invalid());
    }
    let Some((key, name)) = query.split_once('=') else {
        return Err(invalid());
    };
    if key != "name" || service::validate_service_name(name).is_err() {
        return Err(invalid());
    }
    Ok(name)
}

async fn service_beacon_store() -> Result<JobStorage, String> {
    let bucket = targets::GCS_REGISTRY_URI
        .split_once("//")
        .map(|(_, rest)| rest.split('/').next().unwrap_or_default())
        .unwrap_or_default();
    JobStorage::with_bucket(bucket)
        .await
        .map_err(|error| error.to_string())
}

async fn declared_services_matching(name: &str) -> Result<Vec<service::ManagedService>, String> {
    let registry = targets::fetch_registry_remote()
        .await
        .map_err(|error| error.to_string())?;
    let mut found = Vec::new();
    for target in registry.local_targets() {
        found.extend(
            service::declared_services(target)
                .into_iter()
                .filter(|declared| declared.matches(name)),
        );
    }
    Ok(found)
}

fn service_success(result: Value) -> Response {
    send_json(http_status("200"), &json!({"ok": true, "result": result}))
}

fn service_failure(
    status: u16,
    code: &str,
    message: impl Into<String>,
    retryable: bool,
) -> Response {
    send_json(
        status,
        &json!({
            "ok": false,
            "error": {
                "code": code,
                "message": message.into(),
                "retryable": retryable,
            },
        }),
    )
}

fn invalid_service_request(message: impl Into<String>) -> Response {
    service_failure(http_status("400"), "INVALID_REQUEST", message, false)
}

/// Exactly one canonical target and, optionally, one managed binary. Unknown,
/// duplicate, empty, malformed, and lossy query components are refused before
/// target resolution can open a host channel.
fn service_converge_query(query: &str) -> Result<(String, Option<String>), Response> {
    let invalid = || {
        invalid_service_request(
            "query must contain exactly one non-empty target and at most one non-empty binary",
        )
    };
    if query.is_empty() || query.starts_with('&') || query.ends_with('&') {
        return Err(invalid());
    }
    let mut target = None;
    let mut binary = None;
    for pair in query.split('&') {
        let Some((encoded_key, encoded_value)) = pair.split_once('=') else {
            return Err(invalid());
        };
        if encoded_key.is_empty() || encoded_value.is_empty() || encoded_value.contains('=') {
            return Err(invalid());
        }
        let Some(key) = strict_url_decode(encoded_key) else {
            return Err(invalid());
        };
        let Some(value) = strict_url_decode(encoded_value) else {
            return Err(invalid());
        };
        if value.is_empty() || value.trim() != value {
            return Err(invalid());
        }
        match key.as_str() {
            "target" if target.is_none() => target = Some(value),
            "binary" if binary.is_none() => binary = Some(value),
            _ => return Err(invalid()),
        }
    }
    target.map(|target| (target, binary)).ok_or_else(invalid)
}
