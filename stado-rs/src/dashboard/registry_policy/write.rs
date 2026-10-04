//! The write half of the registry-policy route. The janitor reads the same
//! screens make live in `cleanup.rs`.

use super::*;

/// The refusal a client receives for a retired janitor declaration.
const RETIRED_DECLARATION: &str = "disk_cleanup and memory_reclaim are no longer declared; the \
     janitor deletes everything the fleet put on a host at 80% used";

fn refuse(status: reqwest::StatusCode, error: impl Into<String>) -> Response {
    send_json(http_status(status), &json!({"error": error.into()}))
}

/// `POST /api/registry/policy`
///
/// A compare-and-swap over the one operator-writable target field,
/// `pinned_only`: read the current generation, rewrite it, validate the WHOLE
/// document, and swap only if nobody moved it. The returned generation is the
/// operator's proof the write landed on the document they were reading.
pub(crate) async fn set_policy(request: &Request) -> Response {
    let payload: Value = match serde_json::from_slice(&request.body) {
        Ok(payload) => payload,
        Err(error) => {
            return refuse(
                reqwest::StatusCode::BAD_REQUEST,
                format!("cannot read request JSON: {error}"),
            )
        }
    };
    let Some(body) = payload.as_object() else {
        return refuse(
            reqwest::StatusCode::BAD_REQUEST,
            "request must be a JSON object",
        );
    };
    let Some(target) = body.get("target").and_then(Value::as_str) else {
        return refuse(
            reqwest::StatusCode::BAD_REQUEST,
            "request must name a target",
        );
    };
    for key in body.keys() {
        match key.as_str() {
            "target" | "pinned_only" => {}
            "disk_cleanup" | "memory_reclaim" => {
                return refuse(reqwest::StatusCode::BAD_REQUEST, RETIRED_DECLARATION)
            }
            other => {
                return refuse(
                    reqwest::StatusCode::BAD_REQUEST,
                    format!("unsupported key {other:?}"),
                )
            }
        }
    }
    let Some(pinned) = body.get("pinned_only") else {
        return refuse(
            reqwest::StatusCode::BAD_REQUEST,
            "request must carry pinned_only",
        );
    };
    let Some(pinned) = pinned.as_bool() else {
        return refuse(
            reqwest::StatusCode::BAD_REQUEST,
            "pinned_only must be a boolean",
        );
    };

    let store = match crate::targets::RegistryStore::open().await {
        Ok(store) => store,
        Err(error) => {
            return refuse(
                reqwest::StatusCode::SERVICE_UNAVAILABLE,
                format!("registry store unavailable: {error}"),
            )
        }
    };
    let current = match store.read_versioned().await {
        Ok(Some(current)) => current,
        Ok(None) => {
            return refuse(
                reqwest::StatusCode::SERVICE_UNAVAILABLE,
                "canonical registry generation unavailable",
            )
        }
        Err(error) => {
            return refuse(
                reqwest::StatusCode::SERVICE_UNAVAILABLE,
                format!("canonical registry unreadable: {error}"),
            )
        }
    };
    let mut document: Value = match serde_json::from_str(&current.content) {
        Ok(document) => document,
        Err(error) => {
            return refuse(
                reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                format!("canonical registry is not JSON: {error}"),
            )
        }
    };
    let Some(entries) = document.get_mut("targets").and_then(Value::as_array_mut) else {
        return refuse(
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            "registry.targets must be an array",
        );
    };
    let Some(entry) = entries
        .iter_mut()
        .find(|entry| entry.get("name").and_then(Value::as_str) == Some(target))
        .and_then(Value::as_object_mut)
    else {
        return refuse(
            reqwest::StatusCode::NOT_FOUND,
            format!("target not in registry: {target}"),
        );
    };
    entry.insert("pinned_only".to_string(), Value::from(pinned));

    crate::targets::strip_retired_resource_declarations(&mut document);
    if let Err(error) = crate::targets::validate_registry(&document) {
        return refuse(reqwest::StatusCode::BAD_REQUEST, error.to_string());
    }
    let payload = match serde_json::to_string_pretty(&document) {
        Ok(payload) => format!("{payload}\n"),
        Err(error) => {
            return refuse(
                reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                format!("cannot serialize registry: {error}"),
            )
        }
    };
    match store.compare_and_swap(&current.version, &payload).await {
        Ok(generation) => send_json(
            http_status(reqwest::StatusCode::OK),
            &json!({"ok": true, "target": target, "generation": generation}),
        ),
        Err(error) => refuse(
            reqwest::StatusCode::CONFLICT,
            format!("registry moved while writing: {error}"),
        ),
    }
}
