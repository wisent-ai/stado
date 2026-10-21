//! The declared-host plane: one host's beacon publication, and the durable
//! storage-root handoff a host is driven through.

use serde_json::{json, Value};

use crate::dashboard::listener::auth::authorize_host_health;
use crate::dashboard::listener::http::{url_decode, valid_beacon_host};
use crate::dashboard::listener::{
    http_status, parse_qs, send_json, storage_error_response, Dashboard, Request, Response,
};

impl Dashboard {
    pub(crate) async fn storage_root_reconcile(
        &self,
        request: &Request,
        query: &str,
        write: bool,
    ) -> Response {
        // The reader reads exactly the Content-Length it was given, so a
        // declared body is a non-empty one.
        if request.header("transfer-encoding").is_some() || !request.body.is_empty() {
            return send_json(
                http_status(reqwest::StatusCode::BAD_REQUEST),
                &json!({"error": "storage reconciliation does not accept a request body"}),
            );
        }
        let (target, transaction, phase) = match storage_reconciliation_query(query, write) {
            Ok(scope) => scope,
            Err(response) => return response,
        };
        match crate::cli::host::storage_root_reconcile_result(&target, &transaction, &phase).await {
            Ok(result) => {
                let (exit_code, refusal) = match result.outcome {
                    Ok(()) => (nix::libc::EXIT_SUCCESS, None),
                    Err(error) => {
                        // The error's own sentence, which names its exit code
                        // when it carries no message, never an empty refusal.
                        let message = error.to_string();
                        // An error no failure code classifies is the declared
                        // unknown failure.
                        let failure = match error.failure {
                            Some(failure) => failure,
                            None => crate::primitives::failure::FailureCode::Unknown,
                        };
                        let code = if error.code == crate::cli::CLICK_ERROR_CODE {
                            failure.exit_code(error.code)
                        } else {
                            error.code
                        };
                        (code, Some(message))
                    }
                };
                let mut envelope = json!({"exit_code": exit_code, "refusal": refusal});
                envelope["report"] = result.report;
                send_json(http_status(reqwest::StatusCode::OK), &envelope)
            }
            Err(error) => send_json(
                http_status(reqwest::StatusCode::SERVICE_UNAVAILABLE),
                &json!({
                    "error_code": "STORAGE_RECONCILIATION_FAILED",
                    "error": error.to_string(),
                }),
            ),
        }
    }

    pub(crate) async fn put_host_health(&self, request: &Request, query: &str) -> Response {
        match authorize_host_health(self, request).await {
            Ok(true) => {}
            Ok(false) => {
                return send_json(
                    http_status(reqwest::StatusCode::UNAUTHORIZED),
                    &json!({"error": "unauthorized"}),
                )
            }
            // An unreadable authorization item is this service's failure, not
            // the caller's credential. Answering 401 for it told every host in
            // the fleet its beacon grant had been rejected while the real
            // fault was local and retryable, and the beacons stayed silent
            // for seventeen hours behind that sentence.
            Err(unavailable) => {
                return send_json(
                    http_status(reqwest::StatusCode::SERVICE_UNAVAILABLE),
                    &unavailable.body("host-health"),
                )
            }
        }
        let values = match parse_qs(query) {
            Ok(values) => values,
            Err(response) => return response,
        };
        let host = match values.as_slice() {
            [(key, value)] if key == "host" => value.clone(),
            _ => {
                return send_json(
                    http_status(reqwest::StatusCode::BAD_REQUEST),
                    &json!({"error": "exactly one host query parameter is required"}),
                )
            }
        };
        if !valid_beacon_host(&host) {
            return send_json(
                http_status(reqwest::StatusCode::BAD_REQUEST),
                &json!({"error": "host must be a lowercase DNS label"}),
            );
        }
        // A beacon is one JSON document framed by its Content-Length (the
        // reader frames the body by it; a request without one has none).
        let framing = |reason: String| {
            send_json(
                http_status(reqwest::StatusCode::BAD_REQUEST),
                &json!({"error": format!("invalid JSON request framing: {reason}")}),
            )
        };
        let media_type = match request.header("content-type") {
            Some(declared) => match declared.split_once(';') {
                Some((media_type, _)) => media_type.trim(),
                None => declared.trim(),
            },
            None => return framing("the request declares no Content-Type".to_string()),
        };
        if media_type != "application/json" {
            return framing(format!(
                "its Content-Type is {media_type:?}, not application/json"
            ));
        }
        if request.header("content-length").is_none() || request.body.is_empty() {
            return framing(format!(
                "the beacon document must be a non-empty body framed by Content-Length; it carries {} bytes",
                request.body.len()
            ));
        }
        let payload: Value = match serde_json::from_slice(&request.body) {
            Ok(value) => value,
            Err(error) => {
                return send_json(
                    http_status(reqwest::StatusCode::BAD_REQUEST),
                    &json!({"error": format!("invalid JSON: {error}")}),
                )
            }
        };
        let Some(document) = payload.as_object() else {
            return send_json(
                http_status(reqwest::StatusCode::BAD_REQUEST),
                &json!({"error": "host beacon must be a JSON object"}),
            );
        };
        if document.get("host").and_then(Value::as_str) != Some(host.as_str())
            || document
                .get("reported_at")
                .and_then(Value::as_str)
                .is_none()
            || document.get("units").and_then(Value::as_object).is_none()
        {
            return send_json(
                http_status(reqwest::StatusCode::BAD_REQUEST),
                &json!({"error": "beacon host must match the query and reported_at/units are required"}),
            );
        }
        let path = crate::monitor::host_health::beacon_object_path(&host);
        match self.store.upload_bytes(&path, &request.body).await {
            Ok(()) => send_json(
                http_status(reqwest::StatusCode::OK),
                &json!({"state": "stored", "host": host, "path": path}),
            ),
            Err(error) => storage_error_response(error),
        }
    }
}

/// One canonical registry target and no other query authority.
///
/// The handler resolves this name through [`crate::deploy::host_inventory::inventory_host`];
/// it never becomes an SSH address supplied directly by the client.
pub(crate) fn host_inventory_target(query: &str) -> Result<String, Response> {
    let values = parse_qs(query)?;
    match values.as_slice() {
        [(key, target)] if key == "target" && !target.is_empty() => Ok(target.clone()),
        _ => Err(send_json(
            http_status(reqwest::StatusCode::BAD_REQUEST),
            &json!({"error": "exactly one non-empty target is required"}),
        )),
    }
}

fn storage_reconciliation_query(
    query: &str,
    write: bool,
) -> Result<(String, String, String), Response> {
    let invalid = || {
        send_json(
            http_status(reqwest::StatusCode::BAD_REQUEST),
            &json!({
                "error": "query must contain exactly one non-empty target, transaction and phase; GET accepts status, POST accepts run, resume, rollback or finalize"
            }),
        )
    };
    if query.is_empty() || query.starts_with('&') || query.ends_with('&') {
        return Err(invalid());
    }
    let mut target = None;
    let mut transaction = None;
    let mut phase = None;
    for pair in query.split('&') {
        let Some((encoded_key, encoded_value)) = pair.split_once('=') else {
            return Err(invalid());
        };
        if encoded_key.is_empty() || encoded_value.is_empty() || encoded_value.contains('=') {
            return Err(invalid());
        }
        let (key, value) = match (url_decode(encoded_key), url_decode(encoded_value)) {
            (Ok(key), Ok(value)) => (key, value),
            (Err(reason), _) | (_, Err(reason)) => {
                return Err(send_json(
                    http_status(reqwest::StatusCode::BAD_REQUEST),
                    &json!({"error": format!("the query field {pair:?} cannot be read: {reason}")}),
                ))
            }
        };
        if value.is_empty() || value.trim() != value {
            return Err(invalid());
        }
        match key.as_str() {
            "target" if target.is_none() => target = Some(value),
            "transaction" if transaction.is_none() => transaction = Some(value),
            "phase" if phase.is_none() => phase = Some(value),
            _ => return Err(invalid()),
        }
    }
    let (Some(target), Some(transaction), Some(phase)) = (target, transaction, phase) else {
        return Err(invalid());
    };
    let valid_phase = if write {
        crate::deploy::host_storage_reconcile::ResidentAction::parse(&phase).is_ok()
    } else {
        phase == crate::deploy::host_storage_reconcile::STATUS
    };
    if !valid_phase {
        return Err(invalid());
    }
    Ok((target, transaction, phase))
}
