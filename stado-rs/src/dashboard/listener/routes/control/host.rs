//! The declared-host plane: one host's beacon publication, and the durable
//! storage-root handoff a host is driven through.

use serde_json::{json, Value};

use crate::dashboard::listener::auth::authorize_host_health;
use crate::dashboard::listener::http::{strict_url_decode, valid_beacon_host};
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
        if request.header("transfer-encoding").is_some()
            || request.content_length != usize::default()
            || !request.body.is_empty()
        {
            return send_json(
                http_status("400"),
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
                    Ok(()) => (0, None),
                    Err(error) => {
                        let message = error.message.unwrap_or_default();
                        let failure = error
                            .failure
                            .unwrap_or_else(|| crate::failure::classify_message(&message));
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
                send_json(http_status("200"), &envelope)
            }
            Err(error) => send_json(
                http_status("503"),
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
            Ok(false) => return send_json(http_status("401"), &json!({"error": "unauthorized"})),
            // An unreadable authorization item is this service's failure, not
            // the caller's credential. Answering 401 for it told every host in
            // the fleet its beacon grant had been rejected while the real
            // fault was local and retryable, and the beacons stayed silent
            // for seventeen hours behind that sentence.
            Err(()) => {
                return send_json(
                    http_status("503"),
                    &json!({"error": "host-health authorization unavailable"}),
                )
            }
        }
        let values = parse_qs(query);
        let host = match values.as_slice() {
            [(key, value)] if key == "host" => value.clone(),
            _ => {
                return send_json(
                    http_status("400"),
                    &json!({"error": "exactly one host query parameter is required"}),
                )
            }
        };
        if !valid_beacon_host(&host) {
            return send_json(
                http_status("400"),
                &json!({"error": "host must be a lowercase DNS label"}),
            );
        }
        let content_type = request
            .header("content-type")
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim();
        let content_length = request
            .header("content-length")
            .and_then(|value| value.parse::<usize>().ok());
        if content_type != "application/json"
            || content_length != Some(request.body.len())
            || request.body.is_empty()
        {
            return send_json(
                http_status("400"),
                &json!({"error": "invalid JSON request framing"}),
            );
        }
        let payload: Value = match serde_json::from_slice(&request.body) {
            Ok(value) => value,
            Err(error) => {
                return send_json(
                    http_status("400"),
                    &json!({"error": format!("invalid JSON: {error}")}),
                )
            }
        };
        let Some(document) = payload.as_object() else {
            return send_json(
                http_status("400"),
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
                http_status("400"),
                &json!({"error": "beacon host must match the query and reported_at/units are required"}),
            );
        }
        let path = crate::monitor::host_health::beacon_object_path(&host);
        match self.store.upload_bytes(&path, &request.body).await {
            Ok(()) => send_json(
                http_status("200"),
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
    let values = parse_qs(query);
    if values.len() != 1 || values[0].0 != "target" || values[0].1.is_empty() {
        return Err(send_json(
            http_status("400"),
            &json!({"error": "exactly one non-empty target is required"}),
        ));
    }
    Ok(values[0].1.clone())
}

fn storage_reconciliation_query(
    query: &str,
    write: bool,
) -> Result<(String, String, String), Response> {
    let invalid = || {
        send_json(
            http_status("400"),
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
            "transaction" if transaction.is_none() => transaction = Some(value),
            "phase" if phase.is_none() => phase = Some(value),
            _ => return Err(invalid()),
        }
    }
    let (Some(target), Some(transaction), Some(phase)) = (target, transaction, phase) else {
        return Err(invalid());
    };
    let valid_phase = if write {
        matches!(phase.as_str(), "run" | "resume" | "rollback" | "finalize")
    } else {
        phase == "status"
    };
    if !valid_phase {
        return Err(invalid());
    }
    Ok((target, transaction, phase))
}
