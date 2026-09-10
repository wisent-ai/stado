//! The registry and cleanup routes Stado Desktop calls to read and edit a
//! fleet's cleanup policy, run the local janitor, and import registry data.
//!
//! They existed only on the client side until 2026-09-02. `CleanupClient` and
//! `FleetControl` in the desktop app had been written against
//! `api/registry.json`, `api/registry/policy`, `api/cleanup.json` and
//! `api/cleanup/run` for some time, and all four answered `404` on the live
//! dashboard — verified by probing every port this listener binds, and by
//! grepping the whole crate, where `registry/policy` and `cleanup/run` did not
//! appear at all. So the graphical surface could neither show a policy nor
//! change one, while the command line could set exactly one cleaner root.
//!
//! What the projection deliberately does NOT do: it never returns routing or
//! SSH material, and a write accepts only the whitelisted policy keys. The
//! registry document carries a fleet's addresses and channels; an operator
//! client asking about cleanup has no business receiving them, and this file
//! is not a registry editor.

use serde_json::{json, Map, Value};

use super::{constant_time_eq, http_status, send_json, Request, Response};
use crate::config;

mod cleanup;
mod write;

pub(super) use cleanup::{get_cleanup, run_cleanup};
pub(super) use write::set_policy;

/// Policy fields an operator client may read and write.
///
/// The same list on both sides on purpose: a field the GUI can display and
/// cannot change is a control an operator will try to use, and a field it can
/// change and cannot display is a write nobody can verify. `cleaners` is
/// absent from both — a cleaner's root is a path on a host, and paths are the
/// material this projection exists to withhold.
const POLICY_FIELDS: [&str; 8] = [
    "check_interval_seconds",
    "low_free_gb",
    "max_bytes_per_pass",
    "max_items_per_pass",
    "max_pass_seconds",
    "max_scan_items",
    "mode",
    "target_free_gb",
];

/// Memory-policy fields an operator client may read and write.
///
/// Memory repair declarations are editable on the same terms as CLI writes.
/// The complete candidate still passes the canonical registry validator.
pub(super) const MEMORY_POLICY_FIELDS: [&str; 9] = [
    "check_interval_seconds",
    "high_swap_used_pct",
    "low_free_mb",
    "target_free_mb",
    "max_pass_seconds",
    "max_repairs_per_pass",
    "mode",
    "refuse_placement",
    "repairs",
];

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
            http_status("401"),
            &json!({"error": "unauthorized"}),
        )),
        Err(()) => Err(send_json(
            http_status("503"),
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
    if let Some(policy) = entry.get("disk_cleanup").and_then(Value::as_object) {
        let mut whitelisted = Map::new();
        for field in POLICY_FIELDS {
            if let Some(value) = policy.get(field) {
                whitelisted.insert(field.to_string(), value.clone());
            }
        }
        projected.insert("disk_cleanup".to_string(), Value::Object(whitelisted));
    }
    if let Some(policy) = entry.get("memory_reclaim").and_then(Value::as_object) {
        let mut whitelisted = Map::new();
        for field in MEMORY_POLICY_FIELDS {
            if let Some(value) = policy.get(field) {
                whitelisted.insert(field.to_string(), value.clone());
            }
        }
        projected.insert("memory_reclaim".to_string(), Value::Object(whitelisted));
    }
    // The recordings directory is the one path this projection carries,
    // because `host weles-recordings-dir` already exposes it as an operator
    // control and the desktop app displays it beside the policy.
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
                http_status("503"),
                &json!({"error": format!("registry store unavailable: {error}")}),
            )
        }
    };
    let current = match store.read_versioned().await {
        Ok(Some(current)) => current,
        Ok(None) => {
            return send_json(
                http_status("503"),
                &json!({"error": "canonical registry generation unavailable"}),
            )
        }
        Err(error) => {
            return send_json(
                http_status("503"),
                &json!({"error": format!("canonical registry unreadable: {error}")}),
            )
        }
    };
    let document: Value = match serde_json::from_str(&current.content) {
        Ok(document) => document,
        Err(error) => {
            return send_json(
                http_status("500"),
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
        http_status("200"),
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
            http_status("415"),
            &json!({"error": "registry import requires Content-Type: application/json"}),
        );
    }
    match crate::registry_import::import_bytes(&request.body).await {
        Ok(receipt) => {
            let status = match receipt.state.as_str() {
                "imported" | "unchanged" => "200",
                "conflict" => "409",
                "rejected" => "400",
                _ => "500",
            };
            send_json(http_status(status), &json!(receipt))
        }
        Err(error) => send_json(
            http_status("503"),
            &json!({"error": format!("registry import unavailable: {error}")}),
        ),
    }
}
