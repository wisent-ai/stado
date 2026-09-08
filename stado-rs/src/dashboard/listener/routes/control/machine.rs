//! The canonical machine plane: submit, status and cancel, each pinned to a
//! target the authenticated client is allowed to reach.

use serde_json::{json, Value};

use crate::machine::{MachineError, MachineFacade, SCHEMA_VERSION as MACHINE_SCHEMA_VERSION};

use crate::dashboard::listener::auth::{authenticate_machine_client, machine_result_target};
use crate::dashboard::listener::http::MAX_HEAD_BYTES;
use crate::dashboard::listener::{http_status, send_json, Dashboard, Request, Response};

static MACHINE_JOB_ID_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$")
        .expect("static machine job ID regex compiles")
});

impl Dashboard {
    fn machine_facade(&self) -> MachineFacade {
        MachineFacade::with_store(self.store.clone(), self.store.bucket_name().to_string())
    }

    pub(crate) async fn get_machine_status(&self, request: &Request, query: &str) -> Response {
        if request.content_length != usize::default() || !request.body.is_empty() {
            return invalid_machine_request("machine status does not accept a request body");
        }
        let client = match authenticate_machine_client(request, "status").await {
            Ok(Some(client)) => client,
            Ok(None) => {
                return machine_result_response(Err(MachineError::new(
                    "UNAUTHORIZED",
                    "unauthorized",
                )))
            }
            Err(()) => {
                return machine_result_response(Err(MachineError::retryable(
                    "AUTH_UNAVAILABLE",
                    "machine authorization unavailable",
                )))
            }
        };
        let job_id = match machine_job_id(query) {
            Ok(job_id) => job_id,
            Err(response) => return response,
        };
        let result = self.machine_facade().status(job_id).await;
        let target_allowed = result
            .as_ref()
            .ok()
            .and_then(machine_result_target)
            .is_some_and(|target| client.allows_target(target));
        if !target_allowed {
            return machine_result_response(Err(MachineError::new("UNAUTHORIZED", "unauthorized")));
        }
        machine_result_response(result)
    }

    pub(crate) async fn post_machine_submit(&self, request: &Request) -> Response {
        let content_type = request
            .header("content-type")
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim();
        if request.path != "/api/machine/submit"
            || content_type != "application/json"
            || request.header("transfer-encoding").is_some()
            || request.header("content-length").is_none()
            || request.content_length != request.body.len()
            || request.body.len() > MAX_HEAD_BYTES
        {
            return invalid_machine_request("invalid JSON request framing");
        }
        let mut payload: Value = match serde_json::from_slice(&request.body) {
            Ok(payload) => payload,
            Err(error) => {
                return invalid_machine_request(format!("cannot read request JSON: {error}"))
            }
        };
        if let Err(error) = validate_remote_machine_request(&payload) {
            return machine_result_response(Err(error));
        }
        let client = match authenticate_machine_client(request, "submit").await {
            Ok(Some(client)) => client,
            Ok(None) => {
                return machine_result_response(Err(MachineError::new(
                    "UNAUTHORIZED",
                    "unauthorized",
                )))
            }
            Err(()) => {
                return machine_result_response(Err(MachineError::retryable(
                    "AUTH_UNAVAILABLE",
                    "machine authorization unavailable",
                )))
            }
        };
        let requested = payload
            .get("provider")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let target = if requested.is_empty() {
            let [target] = client.targets() else {
                return machine_result_response(Err(MachineError::new(
                    "UNAUTHORIZED",
                    "unauthorized",
                )));
            };
            target.clone()
        } else if client.allows_target(requested) {
            requested.to_string()
        } else {
            return machine_result_response(Err(MachineError::new("UNAUTHORIZED", "unauthorized")));
        };
        let Some(object) = payload.as_object_mut() else {
            return invalid_machine_request("machine request must be an object");
        };
        object.insert("provider".to_string(), Value::String(target));
        object.insert("pin_to_provider".to_string(), Value::Bool(true));
        machine_result_response(self.machine_facade().submit_request(&payload).await)
    }

    pub(crate) async fn post_machine_cancel(&self, request: &Request, query: &str) -> Response {
        if request.header("transfer-encoding").is_some()
            || request.content_length != usize::default()
            || !request.body.is_empty()
        {
            return invalid_machine_request("machine cancel does not accept a request body");
        }
        let client = match authenticate_machine_client(request, "cancel").await {
            Ok(Some(client)) => client,
            Ok(None) => {
                return machine_result_response(Err(MachineError::new(
                    "UNAUTHORIZED",
                    "unauthorized",
                )))
            }
            Err(()) => {
                return machine_result_response(Err(MachineError::retryable(
                    "AUTH_UNAVAILABLE",
                    "machine authorization unavailable",
                )))
            }
        };
        let job_id = match machine_job_id(query) {
            Ok(job_id) => job_id,
            Err(response) => return response,
        };
        let status = self.machine_facade().status(job_id).await;
        let target_allowed = status
            .as_ref()
            .ok()
            .and_then(machine_result_target)
            .is_some_and(|target| client.allows_target(target));
        if !target_allowed {
            return machine_result_response(Err(MachineError::new("UNAUTHORIZED", "unauthorized")));
        }
        machine_result_response(self.machine_facade().cancel_job(job_id).await)
    }
}

pub(crate) fn machine_result_response(result: Result<Value, MachineError>) -> Response {
    match result {
        Ok(result) => send_json(
            http_status("200"),
            &json!({"schema_version": MACHINE_SCHEMA_VERSION, "ok": true, "result": result}),
        ),
        Err(error) => {
            let status = match error.code.as_str() {
                "INVALID_REQUEST" | "INVALID_SOURCE_ARCHIVE" => http_status("400"),
                "NOT_FOUND" => http_status("404"),
                "IDEMPOTENCY_CONFLICT" => http_status("409"),
                "UNAUTHORIZED" => http_status("401"),
                "FORBIDDEN" => http_status("403"),
                _ if error.retryable => http_status("503"),
                _ => http_status("500"),
            };
            send_json(
                status,
                &json!({
                    "schema_version": MACHINE_SCHEMA_VERSION,
                    "ok": false,
                    "error": {
                        "code": error.code,
                        "message": error.message,
                        "retryable": error.retryable,
                    },
                }),
            )
        }
    }
}

fn invalid_machine_request(message: impl Into<String>) -> Response {
    machine_result_response(Err(MachineError::new("INVALID_REQUEST", message)))
}

fn machine_job_id(query: &str) -> Result<&str, Response> {
    let invalid =
        || invalid_machine_request("query must contain exactly one path-safe job_id parameter");
    if query.is_empty() || query.contains('&') {
        return Err(invalid());
    }
    let Some((name, job_id)) = query.split_once('=') else {
        return Err(invalid());
    };
    if name != "job_id" || !MACHINE_JOB_ID_RE.is_match(job_id) {
        return Err(invalid());
    }
    Ok(job_id)
}

fn validate_remote_machine_request(request: &Value) -> Result<(), MachineError> {
    let Some(request) = request.as_object() else {
        return Ok(());
    };
    if request.contains_key("source_archive_path") {
        return Err(MachineError::new(
            "INVALID_REQUEST",
            "source_archive_path is not accepted by the remote machine API; upload through the object API and declare a stado:// input_object",
        ));
    }
    let Some(inputs) = request.get("input_objects").and_then(Value::as_object) else {
        return Ok(());
    };
    for value in inputs.values() {
        let Some(spec) = value.as_object() else {
            continue;
        };
        if spec
            .keys()
            .any(|key| !matches!(key.as_str(), "stado_uri" | "relative_path" | "sha256"))
        {
            return Err(MachineError::new(
                "INVALID_REQUEST",
                "input_objects entries accept only stado_uri, relative_path, and sha256",
            ));
        }
    }
    Ok(())
}
