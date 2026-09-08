//! The write half of the registry-policy route. The janitor reads the same
//! screens make live in `cleanup.rs`.

use super::*;

/// `POST /api/registry/policy`
///
/// A compare-and-swap over the registry policy whitelist: read the current
/// generation, rewrite exactly the named fields, validate the WHOLE document,
/// and swap only if nobody moved it. The returned generation is the operator's
/// proof the write landed on the document they were reading.
pub(crate) async fn set_policy(request: &Request) -> Response {
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
    let requested_memory = body.get("memory_reclaim");
    if pinned_only.is_none() && requested_policy.is_none() && requested_memory.is_none() {
        return send_json(
            http_status("400"),
            &json!({"error": "request must carry pinned_only, disk_cleanup or memory_reclaim"}),
        );
    }
    for key in body.keys() {
        if !matches!(
            key.as_str(),
            "target" | "pinned_only" | "disk_cleanup" | "memory_reclaim"
        ) {
            return send_json(
                http_status("400"),
                &json!({"error": format!("unsupported key {key:?}")}),
            );
        }
    }
    for (name, requested, allowed) in [
        ("disk_cleanup", requested_policy, &POLICY_FIELDS[..]),
        (
            "memory_reclaim",
            requested_memory,
            &MEMORY_POLICY_FIELDS[..],
        ),
    ] {
        let Some(policy) = requested else {
            continue;
        };
        let Some(fields) = policy.as_object() else {
            return send_json(
                http_status("400"),
                &json!({"error": format!("{name} must be an object")}),
            );
        };
        if fields.is_empty() {
            return send_json(
                http_status("400"),
                &json!({"error": format!("{name} must name at least one field")}),
            );
        }
        for key in fields.keys() {
            if !allowed.contains(&key.as_str()) {
                return send_json(
                    http_status("400"),
                    &json!({"error": format!("{name}.{key} is not an operator-writable field")}),
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
    if let Some(fields) = requested_memory.and_then(Value::as_object) {
        // Seeded from the memory reporting default on the same terms, so a
        // first declaration from the GUI starts at `report` with no repair
        // armed rather than at whatever the patch omits.
        let mut policy = match entry.get("memory_reclaim") {
            Some(existing) if existing.is_object() => existing.clone(),
            _ => match serde_json::to_value(
                crate::providers::local::host_memory::MemoryReclaimPolicy::reporting_default(),
            ) {
                Ok(mut seeded) => {
                    strip_nulls(&mut seeded);
                    seeded
                }
                Err(error) => {
                    return send_json(
                        http_status("500"),
                        &json!({"error": format!("default memory policy unavailable: {error}")}),
                    )
                }
            },
        };
        let Some(policy_map) = policy.as_object_mut() else {
            return send_json(
                http_status("500"),
                &json!({"error": "registry target memory_reclaim must be an object"}),
            );
        };
        for (key, value) in fields {
            if value.is_null() {
                policy_map.remove(key);
            } else {
                policy_map.insert(key.clone(), value.clone());
            }
        }
        entry.insert("memory_reclaim".to_string(), policy);
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
pub(super) fn strip_nulls(value: &mut Value) {
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
