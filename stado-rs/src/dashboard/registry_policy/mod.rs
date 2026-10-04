//! The registry and cleanup routes Stado Desktop calls to read a fleet's
//! targets, pin one, run the local janitor, and import registry data.
//!
//! What the projection deliberately does NOT do: it never returns routing or
//! SSH material, and a write accepts only `pinned_only`. The registry document
//! carries a fleet's addresses and channels; an operator client asking about
//! a host has no business receiving them, and this file is not a registry
//! editor. Janitor behaviour is not declared at all: the disk-full rule
//! ([`crate::providers::local::disk_cleanup::rule`]) has no settings.

use serde_json::{json, Map, Value};

use super::{constant_time_eq, http_status, send_json, Request, Response};
use crate::config;

mod cleanup;
mod write;

pub(super) use cleanup::{get_cleanup, run_cleanup};
pub(super) use write::set_policy;

/// Authenticate one registry-API client bearer for `action`.
///
/// `Ok(None)` is "no client presented a bearer that matches this action", and
/// it is also what an undeclared boundary produces: `registry_api.clients`
/// empty means the loop has nothing to compare against, so the route refuses
/// with `401`. `Err(())` is reserved for a declaration that cannot be read,
/// which is an outage and answers `503`.
pub(super) async fn authenticate(
    request: &Request,
    action: &str,
) -> Result<Option<&'static config::RegistryApiClient>, ()> {
    let Some(supplied) = request
        .header("authorization")
        .and_then(|value| value.trim().strip_prefix("Bearer "))
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    let clients = config::registry_api_clients().map_err(|_| ())?;
    let mut matched = None;
    for client in clients
        .values()
        .filter(|client| client.allows_action(action))
    {
        let expected = crate::skarbiec::read_registry_token(client.item(), "token")
            .await
            .map_err(|_| ())?
            .filter(|value| !value.is_empty())
            .ok_or(())?;
        if constant_time_eq(expected.as_bytes(), supplied.as_bytes()) {
            // Two clients sharing one bearer means neither identity is
            // established, so the request is refused rather than attributed.
            if matched.is_some() {
                return Ok(None);
            }
            matched = Some(client);
        }
    }
    Ok(matched)
}

/// Gate one request on this boundary, or hand back the refusal to send.
pub(super) async fn authorized(request: &Request, action: &str) -> Result<(), Response> {
    match authenticate(request, action).await {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(send_json(
            http_status(reqwest::StatusCode::UNAUTHORIZED),
            &json!({"error": "unauthorized"}),
        )),
        Err(()) => Err(send_json(
            http_status(reqwest::StatusCode::SERVICE_UNAVAILABLE),
            &json!({"error": "registry authorization unavailable"}),
        )),
    }
}

/// One target as an operator client may see it.
fn project_target(entry: &Value) -> Option<Value> {
    let entry = entry.as_object()?;
    let name = entry.get("name").and_then(Value::as_str)?;
    let mut projected = Map::new();
    projected.insert("name".to_string(), Value::from(name));
    if let Some(pinned) = entry.get("pinned_only").and_then(Value::as_bool) {
        projected.insert("pinned_only".to_string(), Value::from(pinned));
    }
    // The work root is the one path this projection carries besides the
    // recordings directory: the disk-full rule is measured on the volume it
    // names.
    if let Some(root) = entry.get("work_root").and_then(Value::as_str) {
        projected.insert("work_root".to_string(), Value::from(root));
    }
    // The recordings directory, because `host weles-recordings-dir` exposes
    // it as an operator control and the janitor sweeps it.
    if let Some(directory) = entry
        .get("weles")
        .and_then(Value::as_object)
        .and_then(|weles| weles.get("recordings_dir"))
        .and_then(Value::as_str)
    {
        projected.insert("weles".to_string(), json!({"recordings_dir": directory}));
    }
    Some(Value::Object(projected))
}

/// `GET /api/registry.json`
pub(super) async fn get_policy() -> Response {
    let store = match crate::targets::RegistryStore::open().await {
        Ok(store) => store,
        Err(error) => {
            return send_json(
                http_status(reqwest::StatusCode::SERVICE_UNAVAILABLE),
                &json!({"error": format!("registry store unavailable: {error}")}),
            )
        }
    };
    let current = match store.read_versioned().await {
        Ok(Some(current)) => current,
        Ok(None) => {
            return send_json(
                http_status(reqwest::StatusCode::SERVICE_UNAVAILABLE),
                &json!({"error": "canonical registry generation unavailable"}),
            )
        }
        Err(error) => {
            return send_json(
                http_status(reqwest::StatusCode::SERVICE_UNAVAILABLE),
                &json!({"error": format!("canonical registry unreadable: {error}")}),
            )
        }
    };
    let document: Value = match serde_json::from_str(&current.content) {
        Ok(document) => document,
        Err(error) => {
            return send_json(
                http_status(reqwest::StatusCode::INTERNAL_SERVER_ERROR),
                &json!({"error": format!("canonical registry is not JSON: {error}")}),
            )
        }
    };
    let targets: Vec<Value> = document
        .get("targets")
        .and_then(Value::as_array)
        .map(|entries| entries.iter().filter_map(project_target).collect())
        .unwrap_or_default();
    // The public-origin declarations are a top-level block, and Desktop
    // renders them beside the report `stado web origin status` produces. An
    // absent key is an empty list: a fleet may publish nothing publicly, and
    // that is a different statement from a projection that dropped the key.
    let public_origins = match document.get(crate::public_origin::POLICY_KEY) {
        Some(declared) => declared.clone(),
        None => Value::Array(Vec::new()),
    };
    send_json(
        http_status(reqwest::StatusCode::OK),
        &json!({
            "generation": current.version,
            "targets": targets,
            "public_origins": public_origins,
        }),
    )
}
/// `POST /api/registry/import`
///
/// The body is the existing registry-v2 document itself, not an envelope, so
/// every caller feeds the exact same bytes to the product-owned import
/// operation. The route reports semantic rejection and conflicts as typed
/// receipts; operational storage failures remain service failures.
pub(super) async fn import_registry(request: &Request) -> Response {
    let content_type = request
        .header("content-type")
        .and_then(|value| value.split(';').next())
        .map(str::trim);
    if !matches!(content_type, Some(value) if value.eq_ignore_ascii_case("application/json")) {
        return send_json(
            http_status(reqwest::StatusCode::UNSUPPORTED_MEDIA_TYPE),
            &json!({"error": "registry import requires Content-Type: application/json"}),
        );
    }
    match crate::registry_import::import_bytes(&request.body).await {
        Ok(receipt) => {
            let status = match receipt.state.as_str() {
                "imported" | "unchanged" => reqwest::StatusCode::OK,
                "conflict" => reqwest::StatusCode::CONFLICT,
                "rejected" => reqwest::StatusCode::BAD_REQUEST,
                _ => reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            };
            send_json(http_status(status), &json!(receipt))
        }
        Err(error) => send_json(
            http_status(reqwest::StatusCode::SERVICE_UNAVAILABLE),
            &json!({"error": format!("registry import unavailable: {error}")}),
        ),
    }
}
