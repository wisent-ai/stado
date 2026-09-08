//! The strict whitelist a machine request must satisfy before anything is
//! reserved for it.

use std::path::Path;

use serde_json::{Map, Value};

use crate::config;
use crate::machine::MachineError;

use super::{
    APT_PACKAGE_RE, ENV_NAME_RE, HOSTNAME_RE, REPO_REF_RE, REQUEST_FIELDS, REQUEST_ID_RE,
    SECRET_PART_RE,
};

/// Python `_validate_request`: strict field whitelist, required fields,
/// per-field types and value rules. Returns the normalized request (defaults
/// merged) on success.
pub fn validate_request(request: &Value) -> Result<Map<String, Value>, MachineError> {
    fn invalid(msg: impl Into<String>) -> MachineError {
        MachineError::new("INVALID_REQUEST", msg)
    }
    let Value::Object(map) = request else {
        return Err(invalid("request file must contain one JSON object"));
    };
    let mut unknown: Vec<&str> = map
        .keys()
        .map(String::as_str)
        .filter(|key| !REQUEST_FIELDS.contains(key))
        .collect();
    unknown.sort_unstable();
    if !unknown.is_empty() {
        return Err(invalid(format!(
            "unknown request field(s): {}",
            unknown.join(", ")
        )));
    }
    let missing: Vec<&str> = ["client_request_id", "command"]
        .into_iter()
        .filter(|name| !map.contains_key(*name))
        .collect();
    if !missing.is_empty() {
        return Err(invalid(format!(
            "missing required field(s): {}",
            missing.join(", ")
        )));
    }

    let request_id = &map["client_request_id"];
    let command = &map["command"];
    if !request_id
        .as_str()
        .is_some_and(|id| REQUEST_ID_RE.is_match(id))
    {
        return Err(invalid(
            "client_request_id must be 1-128 path-safe ASCII characters",
        ));
    }
    if command.as_str().is_none_or(|cmd| cmd.trim().is_empty()) {
        return Err(invalid("command must be a non-empty string"));
    }

    let mut normalized = Map::new();
    // Stado owns provider selection unless the caller supplies a constraint.
    normalized.insert("provider".into(), Value::from(""));
    normalized.insert("gpu_type".into(), Value::from(""));
    normalized.insert("pinned_host".into(), Value::from(""));
    normalized.insert("vram_gb".into(), Value::from(0));
    normalized.insert("max_cost_per_hour_usd".into(), Value::from(0.0));
    normalized.insert("pin_to_provider".into(), Value::from(false));
    normalized.insert("priority".into(), Value::from(0));
    normalized.insert("repo".into(), Value::from(""));
    normalized.insert("repo_ref".into(), Value::from(""));
    normalized.insert("repo_workdir".into(), Value::from(""));
    normalized.insert("repo_extras".into(), Value::from("train"));
    normalized.insert("pre_command".into(), Value::from(""));
    normalized.insert("apt_packages".into(), Value::Array(vec![]));
    normalized.insert("output_uri".into(), Value::from(""));
    normalized.insert("verify_command".into(), Value::from(""));
    normalized.insert("exclusive".into(), Value::from(false));
    normalized.insert("source_archive_path".into(), Value::from(""));
    normalized.insert("input_objects".into(), Value::Object(Map::new()));
    normalized.insert("secret_env".into(), Value::Object(Map::new()));
    for (key, value) in map {
        normalized.insert(key.clone(), value.clone());
    }

    for name in [
        "provider",
        "gpu_type",
        "pinned_host",
        "repo",
        "repo_ref",
        "repo_workdir",
        "repo_extras",
        "pre_command",
        "output_uri",
        "verify_command",
        "source_archive_path",
    ] {
        if !normalized[name].is_string() {
            return Err(invalid(format!("{name} must be a string")));
        }
    }
    let pinned_host = normalized["pinned_host"].as_str().unwrap_or_default();
    if !pinned_host.is_empty() && !HOSTNAME_RE.is_match(pinned_host) {
        return Err(invalid(
            "pinned_host must be a lowercase consumer host name (letters, digits, dot, dash)",
        ));
    }
    let repo = normalized["repo"].as_str().unwrap_or_default();
    let repo_ref = normalized["repo_ref"].as_str().unwrap_or_default();
    if repo.is_empty() {
        if !repo_ref.is_empty() {
            return Err(invalid("repo_ref is valid only when repo is set"));
        }
    } else if !REPO_REF_RE.is_match(repo_ref) {
        return Err(invalid(
            "repo_ref is required with repo and must be a full 40-character lowercase hexadecimal commit",
        ));
    }
    // Python rejects bool explicitly because bool is an int subclass; JSON
    // booleans never deserialize as i64/f64 here, so as_i64/as_f64 suffices.
    for name in ["vram_gb", "priority"] {
        if normalized[name].as_i64().is_none() {
            return Err(invalid(format!("{name} must be an integer")));
        }
    }
    if normalized["vram_gb"].as_i64().unwrap_or_default() < 0 {
        return Err(invalid("vram_gb must not be negative"));
    }
    let Some(cost) = normalized["max_cost_per_hour_usd"].as_f64() else {
        return Err(invalid("max_cost_per_hour_usd must be non-negative"));
    };
    if cost < 0.0 {
        return Err(invalid("max_cost_per_hour_usd must be non-negative"));
    }
    // Python normalizes to float so the digest sees "1.0", not "1".
    normalized.insert("max_cost_per_hour_usd".into(), Value::from(cost));
    for name in ["pin_to_provider", "exclusive"] {
        if !normalized[name].is_boolean() {
            return Err(invalid(format!("{name} must be a boolean")));
        }
    }
    let packages = &normalized["apt_packages"];
    let valid_packages = packages.as_array().is_some_and(|items| {
        items
            .iter()
            .all(|item| item.as_str().is_some_and(|s| APT_PACKAGE_RE.is_match(s)))
    });
    if !valid_packages {
        return Err(invalid(
            "apt_packages must contain only valid apt package names",
        ));
    }
    let Some(secret_env) = normalized["secret_env"].as_object() else {
        return Err(invalid("secret_env must be an object"));
    };
    for (env_name, value) in secret_env {
        if !ENV_NAME_RE.is_match(env_name) {
            return Err(invalid(format!(
                "secret_env variable name is unsafe: {env_name:?}"
            )));
        }
        let Some(spec) = value.as_object() else {
            return Err(invalid(format!("secret_env.{env_name} must be an object")));
        };
        if spec.keys().any(|key| key != "item" && key != "field") {
            return Err(invalid(format!(
                "secret_env.{env_name} accepts only item and field"
            )));
        }
        let item = spec.get("item").and_then(Value::as_str).unwrap_or_default();
        let field = spec
            .get("field")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !SECRET_PART_RE.is_match(item) || !SECRET_PART_RE.is_match(field) {
            return Err(invalid(format!(
                "secret_env.{env_name} requires path-safe item and field strings"
            )));
        }
        if !config::agent_secret_reference_allowed(item, field) {
            return Err(invalid(format!(
                "secret_env.{env_name} reference is not in agent.skarbiec.secret_fields"
            )));
        }
    }
    let Some(inputs) = normalized["input_objects"].as_object() else {
        return Err(invalid("input_objects must be an object"));
    };
    for (name, value) in inputs {
        let Some(spec) = value.as_object() else {
            return Err(invalid(format!("input_objects.{name} must be an object")));
        };
        let Some(uri) = spec.get("stado_uri").and_then(Value::as_str) else {
            return Err(invalid(format!(
                "input_objects.{name}.stado_uri is required"
            )));
        };
        crate::object_store::ObjectRef::parse(uri).map_err(|error| {
            invalid(format!(
                "input_objects.{name}.stado_uri is invalid: {error}"
            ))
        })?;
        let Some(relative) = spec.get("relative_path").and_then(Value::as_str) else {
            return Err(invalid(format!(
                "input_objects.{name}.relative_path is required"
            )));
        };
        let relative_path = Path::new(relative);
        if relative_path.as_os_str().is_empty()
            || relative_path
                .components()
                .any(|part| !matches!(part, std::path::Component::Normal(_)))
        {
            return Err(invalid(format!(
                "input_objects.{name}.relative_path must stay inside the job work directory"
            )));
        }
    }
    Ok(normalized)
}
