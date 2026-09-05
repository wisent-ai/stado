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
    send_json(
        http_status("200"),
        &json!({"generation": current.version, "targets": targets}),
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

/// `POST /api/registry/policy`
///
/// The same compare-and-swap `stado host disk-cleanup` performs, over the same
/// whitelist: read the current generation, rewrite exactly the named fields,
/// validate the WHOLE document, and swap only if nobody moved it. The returned
/// generation is the operator's proof the write landed on the document they
/// were reading.
pub(super) async fn set_policy(request: &Request) -> Response {
    let payload: Value = match serde_json::from_slice(&request.body) {
        Ok(payload) => payload,
        Err(error) => {
            return send_json(
                http_status("400"),
                &json!({"error": format!("cannot read request JSON: {error}")}),
            )
        }
    };
    let Some(body) = payload.as_object() else {
        return send_json(
            http_status("400"),
            &json!({"error": "request must be a JSON object"}),
        );
    };
    let Some(target) = body.get("target").and_then(Value::as_str) else {
        return send_json(
            http_status("400"),
            &json!({"error": "request must name a target"}),
        );
    };
    let pinned_only = body.get("pinned_only");
    let requested_policy = body.get("disk_cleanup");
    if pinned_only.is_none() && requested_policy.is_none() {
        return send_json(
            http_status("400"),
            &json!({"error": "request must carry pinned_only or disk_cleanup"}),
        );
    }
    for key in body.keys() {
        if !matches!(key.as_str(), "target" | "pinned_only" | "disk_cleanup") {
            return send_json(
                http_status("400"),
                &json!({"error": format!("unsupported key {key:?}")}),
            );
        }
    }
    if let Some(policy) = requested_policy {
        let Some(fields) = policy.as_object() else {
            return send_json(
                http_status("400"),
                &json!({"error": "disk_cleanup must be an object"}),
            );
        };
        if fields.is_empty() {
            return send_json(
                http_status("400"),
                &json!({"error": "disk_cleanup must name at least one field"}),
            );
        }
        for key in fields.keys() {
            if !POLICY_FIELDS.contains(&key.as_str()) {
                return send_json(
                    http_status("400"),
                    &json!({"error": format!("disk_cleanup.{key} is not an operator-writable field")}),
                );
            }
        }
    }

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
    let mut document: Value = match serde_json::from_str(&current.content) {
        Ok(document) => document,
        Err(error) => {
            return send_json(
                http_status("500"),
                &json!({"error": format!("canonical registry is not JSON: {error}")}),
            )
        }
    };
    let Some(entries) = document.get_mut("targets").and_then(Value::as_array_mut) else {
        return send_json(
            http_status("500"),
            &json!({"error": "registry.targets must be an array"}),
        );
    };
    let Some(entry) = entries
        .iter_mut()
        .find(|entry| entry.get("name").and_then(Value::as_str) == Some(target))
        .and_then(Value::as_object_mut)
    else {
        return send_json(
            http_status("404"),
            &json!({"error": format!("target not in registry: {target}")}),
        );
    };
    if let Some(pinned) = pinned_only {
        let Some(pinned) = pinned.as_bool() else {
            return send_json(
                http_status("400"),
                &json!({"error": "pinned_only must be a boolean"}),
            );
        };
        entry.insert("pinned_only".to_string(), Value::from(pinned));
    }
    if let Some(fields) = requested_policy.and_then(Value::as_object) {
        // A host that declares no policy is seeded from the reporting default
        // before the named fields apply, exactly as the CLI setter does, so a
        // first declaration from the GUI starts at `report` rather than at
        // whatever the patch omits.
        let mut policy = match entry.get("disk_cleanup") {
            Some(existing) if existing.is_object() => existing.clone(),
            _ => match serde_json::to_value(crate::targets::DiskCleanupPolicy::reporting_default())
            {
                Ok(mut seeded) => {
                    strip_nulls(&mut seeded);
                    seeded
                }
                Err(error) => {
                    return send_json(
                        http_status("500"),
                        &json!({"error": format!("default cleanup policy unavailable: {error}")}),
                    )
                }
            },
        };
        let Some(policy_map) = policy.as_object_mut() else {
            return send_json(
                http_status("500"),
                &json!({"error": "registry target disk_cleanup must be an object"}),
            );
        };
        for (key, value) in fields {
            if value.is_null() {
                policy_map.remove(key);
            } else {
                policy_map.insert(key.clone(), value.clone());
            }
        }
        entry.insert("disk_cleanup".to_string(), policy);
    }

    if let Err(error) = crate::targets::validate_registry(&document) {
        return send_json(http_status("400"), &json!({"error": error.to_string()}));
    }
    let payload = match serde_json::to_string_pretty(&document) {
        Ok(payload) => format!("{payload}\n"),
        Err(error) => {
            return send_json(
                http_status("500"),
                &json!({"error": format!("cannot serialize registry: {error}")}),
            )
        }
    };
    match store.compare_and_swap(&current.version, &payload).await {
        Ok(generation) => send_json(
            http_status("200"),
            &json!({"ok": true, "target": target, "generation": generation}),
        ),
        Err(error) => send_json(
            http_status("409"),
            &json!({"error": format!("registry moved while writing: {error}")}),
        ),
    }
}

/// `serde` writes `Option::None` as `null`, and the cleaner schema accepts a
/// key list rather than nulls, so a seeded default is stripped before it is
/// validated.
fn strip_nulls(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.retain(|_, entry| !entry.is_null());
            for entry in map.values_mut() {
                strip_nulls(entry);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(strip_nulls),
        _ => {}
    }
}

/// The janitor's last recorded pass, sanitized.
fn last_report() -> Value {
    let home = crate::config_file::expand_tilde("~");
    let path = home.join(crate::providers::local::disk_cleanup::state_relative_path());
    let recorded = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|state| state.get("report").cloned())
        .unwrap_or(Value::Null);
    crate::providers::local::disk_cleanup::sanitize_cleanup_report(&recorded)
}

/// `GET /api/cleanup.json`
pub(super) fn get_cleanup() -> Response {
    send_json(
        http_status("200"),
        &json!({"ok": true, "service": "disk-cleanup", "report": last_report()}),
    )
}

/// `POST /api/cleanup/run`
///
/// One interval-gated pass through the janitor's own entry point, so the mode,
/// the watermarks, the budgets and the lock are the registry's — a run asked
/// for from the GUI is the same pass the timer would have made, not a second
/// implementation of it.
pub(super) async fn run_cleanup() -> Response {
    let report = crate::providers::local::disk_cleanup::run_cleanup_once(
        0,
        false,
        crate::providers::local::disk_cleanup::CleanupWriter::Cli,
        &mut |_message| {},
    )
    .await;
    send_json(
        http_status("200"),
        &json!({
            "ok": true,
            "service": "disk-cleanup",
            "report": crate::providers::local::disk_cleanup::sanitize_cleanup_report(&report),
        }),
    )
}
