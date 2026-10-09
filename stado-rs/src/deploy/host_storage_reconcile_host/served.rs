//! What the host's object API serves: its runtime state, and which physical
//! root (A, B or both identically) its `probierz` objects come from, by
//! hashing every served body against the preflight inventories.

use std::collections::BTreeMap;
use std::io::Read;

use base64::Engine;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use super::home;

const LIFECYCLE_PREFIX: &str = "ecosystem/probierz/";

fn origin(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

/// Percent-encode everything but the unreserved characters, as a URI query
/// value that may itself contain `:` and `/`.
fn encode_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// `/api/state.json` of the object API on the loopback port, as the
/// transaction's payload line.
pub(super) async fn object_runtime(port: u16) -> Result<(), String> {
    let url = format!("{}/api/state.json", origin(port));
    let answer = async {
        let response = reqwest::get(&url).await?.error_for_status()?;
        response.json::<Value>().await
    }
    .await;
    match answer {
        Ok(state) => println!("STADO_STORAGE_RECONCILE\t{state}"),
        Err(error) => println!(
            "STADO_STORAGE_RECONCILE_ERROR\tobject API runtime state at {url} is unreadable: {}",
            error.to_string().replace(['\t', '\n'], " ")
        ),
    }
    Ok(())
}

/// Path under `ecosystem/probierz/` to body identity, for one inventory.
fn identities(payload: &Value, name: &str) -> BTreeMap<String, Value> {
    let mut result = BTreeMap::new();
    for item in payload[name].as_array().into_iter().flatten() {
        let path = item.get("path").and_then(Value::as_str).unwrap_or_default();
        if let Some(key) = path.strip_prefix(LIFECYCLE_PREFIX) {
            result.insert(
                key.to_string(),
                item.get("body").cloned().unwrap_or(Value::Null),
            );
        }
    }
    result
}

fn physical_identity(payload: &Value, name: &str, path: &str) -> Value {
    payload[name]["files"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|item| item.get("path").and_then(Value::as_str) == Some(path))
        .and_then(|item| item.get("body").cloned())
        .unwrap_or(Value::Null)
}

fn served_matches(
    keys: &[String],
    served: &Map<String, Value>,
    root: &BTreeMap<String, Value>,
) -> bool {
    keys.iter().eq(root.keys()) && keys.iter().all(|key| served.get(key) == root.get(key))
}

/// Read every object the API lists and name the physical root it serves.
/// The preflight inventories arrive base64-encoded on stdin.
pub(super) async fn served_store(port: u16) -> Result<(), String> {
    let mut encoded = String::new();
    std::io::stdin()
        .read_to_string(&mut encoded)
        .map_err(|error| format!("cannot read the served-store inventory: {error}"))?;
    let payload: Value = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .map_err(|error| error.to_string())
        .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|error| error.to_string()))
        .map_err(|error| format!("served-store inventory is invalid: {error}"))?;
    let home = home()?;
    let token_path = format!("{home}/.stado/queue-object-api-token");
    let token = std::fs::read_to_string(&token_path)
        .map_err(|error| format!("cannot read {token_path}: {error}"))?
        .trim()
        .to_string();
    if token.is_empty() {
        return Err("object API correlation token is empty".to_string());
    }
    let base = origin(port);
    let client = reqwest::Client::new();
    let get = |url: String| {
        crate::wait::send(
            crate::wait::Kind::ObjectApi,
            "object API",
            client.get(url).bearer_auth(&token),
        )
    };
    let failed = |error: reqwest::Error| format!("object API read failed: {error}");
    let listed: Value = get(format!("{base}/api/object/list?namespace=probierz&prefix="))
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(failed)?
        .json()
        .await
        .map_err(failed)?;
    let mut keys: Vec<String> = listed["objects"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("key").and_then(Value::as_str).map(str::to_string))
        .collect();
    keys.sort();
    let primary_before = identities(&payload, "primary");
    let backup = identities(&payload, "backup");
    let mut primary = primary_before.clone();
    if payload["primary_after_commit"].as_bool() == Some(true) {
        if payload["conflict_winner"].as_str() == Some("primary") {
            primary = backup.clone();
            primary.extend(primary_before);
        } else {
            primary.extend(backup.clone());
        }
    }
    let mut served = Map::new();
    for key in &keys {
        let uri = format!("stado://probierz/{key}");
        let url = format!("{base}/api/object?uri={}", encode_component(&uri));
        let mut response = get(url)
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(failed)?;
        let mut digest = Sha256::new();
        let mut size: u64 = 0;
        while let Some(chunk) = response.chunk().await.map_err(failed)? {
            digest.update(&chunk);
            size += chunk.len() as u64;
        }
        served.insert(
            key.clone(),
            json!({"sha256": hex::encode(digest.finalize()), "bytes": size}),
        );
    }
    let matches_primary = served_matches(&keys, &served, &primary);
    let matches_backup = served_matches(&keys, &served, &backup);
    let authority = match (matches_primary, matches_backup) {
        (true, true) => "identical",
        (true, false) => "A",
        (false, true) => "B",
        (false, false) => {
            return Err(
                "object API does not serve either complete physical qualified root".to_string(),
            )
        }
    };
    let objects: Vec<Value> = keys
        .iter()
        .map(|key| {
            json!({
                "backend": "stado-object-api", "namespace": crate::config::QUEUE_OBJECT_NAMESPACE, "key": key,
                "physical_path": format!("{LIFECYCLE_PREFIX}{key}"), "identity": served[key],
            })
        })
        .collect();
    let registry = |root: &str, name: &str| {
        json!({
            "root": root, "backend": "local", "namespace": null, "key": "registry.json",
            "physical_path": "registry.json",
            "identity": physical_identity(&payload, name, "registry.json"),
        })
    };
    let evidence = json!({
        "object_authority": authority,
        "endpoint": base,
        "object_store": {"backend": "stado-object-api", "namespace": crate::config::QUEUE_OBJECT_NAMESPACE, "objects": objects},
        "registry_store": {"mappings": [
            registry("A", "primary_physical"),
            registry("B", "backup_physical"),
            {"root": "served", "backend": "stado-object", "namespace": null,
             "key": "registry.json", "physical_path": null,
             "observation": "client namespace was not observable from the object API"},
        ]},
        "primary_root": format!("{home}/.stado/local-storage"),
        "backup_root": format!("{home}/.stado/local-backup"),
    });
    println!("STADO_SERVED_STORE\t{evidence}");
    Ok(())
}
