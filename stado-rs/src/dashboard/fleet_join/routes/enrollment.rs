//! The enrollment write: one pending request filed under `enrollments/`,
//! paid for with one use of the invite and refunded if it cannot be recorded.

use std::time::Instant;

use serde_json::{json, Value};

use crate::dashboard::{http_status, json_dumps_sorted_compact, send_json, Request, Response};
use crate::queue::{JobStorage, StorageError};
use crate::targets::normalize_hostname;

use super::super::redeem::{refund, spend, verify};
use super::super::refusals::{denied, refuse, unavailable};
use super::super::window::accept_request;
use super::super::{presented, request_path, MAX_REQUEST_BYTES, STATUS_PENDING};
use super::report::{parse_report, valid_hostname};

/// `POST /api/fleet/join` — record the machine's pending enrollment request
/// and consume one use of the invite. Writes only under `enrollments/`; the
/// registry is untouched until an operator approves, and approval re-probes
/// the machine over the channel this request names.
pub(in crate::dashboard) async fn join(store: &JobStorage, request: &Request) -> Response {
    let started = Instant::now();
    let token = presented(request);
    if !accept_request(token.as_ref().map(|(id, _)| id.as_str()), request.peer) {
        return refuse(started).await;
    }
    let content_type = request
        .header("content-type")
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .unwrap_or_default();
    if content_type != "application/json" {
        return send_json(
            http_status("400"),
            &json!({"error": "join report requires Content-Type: application/json"}),
        );
    }
    if request.body.len() > MAX_REQUEST_BYTES {
        return send_json(
            http_status("413"),
            &json!({"error": "join report is too large"}),
        );
    }
    let report = match parse_report(&request.body) {
        Ok(report) => report,
        Err(message) => return send_json(http_status("400"), &json!({"error": message})),
    };
    let hostname = normalize_hostname(&report.hostname);
    if !valid_hostname(&hostname) {
        return send_json(
            http_status("400"),
            &json!({"error": "join report hostname is not a usable machine name"}),
        );
    }
    let Some((id, secret)) = token else {
        return refuse(started).await;
    };
    let accepted = match verify(store, &id, &secret).await {
        Ok(accepted) => accepted,
        Err(denial) => return denied(started, denial).await,
    };

    // A machine whose request was already decided is never silently reopened
    // by a code; only a still-pending request is replaced.
    let path = request_path(&hostname);
    let existing = match store.read_text_versioned(&path).await {
        Ok(existing) => existing,
        Err(_) => return unavailable("enrollment store is unavailable"),
    };
    if let Some(current) = &existing {
        let status = serde_json::from_str::<Value>(&current.content)
            .ok()
            .and_then(|document| {
                document
                    .get("status")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default();
        if status != STATUS_PENDING {
            return send_json(
                http_status("409"),
                &json!({
                    "error": format!(
                        "machine '{hostname}' already has a '{status}' enrollment request"
                    )
                }),
            );
        }
    }

    let spent = match spend(store, &accepted).await {
        Ok(spent) => spent,
        Err(denial) => return denied(started, denial).await,
    };

    let mut document = crate::cli::fleet::enroll::build_invited_request(
        &hostname,
        &report.os,
        &report.arch,
        &accepted.invite.target_name,
        &report.destination,
        &accepted.invite.id,
        &report.fingerprint,
    );
    if let Some(listening) = report.ssh_listening {
        document["ssh_listening"] = json!(listening);
    }
    let body = json_dumps_sorted_compact(&document);
    let recorded = match &existing {
        Some(current) => store
            .compare_and_swap_text(&path, &current.version, &body)
            .await
            .map(|_| true),
        None => store.create_text_if_absent(&path, &body).await,
    };
    match recorded {
        Ok(true) => {}
        Ok(false) | Err(StorageError::StorageConflict(_)) => {
            refund(store, &accepted, &spent).await;
            return send_json(
                http_status("409"),
                &json!({"error": format!("machine '{hostname}' is already enrolling")}),
            );
        }
        Err(_) => {
            refund(store, &accepted, &spent).await;
            return unavailable("enrollment store is unavailable");
        }
    }
    send_json(
        http_status("200"),
        &json!({
            "status": STATUS_PENDING,
            "hostname": hostname,
            "target_name": accepted.invite.target_name,
            "next_step": format!("an operator approves with: stado fleet approve {hostname}"),
        }),
    )
}
