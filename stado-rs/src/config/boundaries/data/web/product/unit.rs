//! Consumer, ready path, edge, environment and database of one web product.

use std::collections::BTreeMap;

use super::WebApiDatabase;
use crate::config::{canonical_machine_name, is_env_name, parse_secret_reference, WEB_API_EDGES};
use serde_json::{Map, Value};

/// The unit half of one declaration: the identity it runs as, its ready path,
/// its edge, and the environment its unit is delivered with.
pub(super) struct WebApiUnit {
    pub(super) consumer: String,
    pub(super) readyz: String,
    pub(super) edge: String,
    pub(super) env: BTreeMap<String, String>,
    pub(super) secrets: BTreeMap<String, String>,
    pub(super) database: Option<WebApiDatabase>,
}

pub(super) fn parse_web_api_unit(
    name: &str,
    entry: &Map<String, Value>,
    redirect_to: Option<&str>,
    upstream_service: Option<&str>,
    problems: &mut Vec<String>,
) -> WebApiUnit {
    let consumer = match entry.get("consumer").and_then(Value::as_str) {
        Some(consumer) if canonical_machine_name(consumer) => consumer.to_string(),
        Some(other) => {
            problems.push(format!(
                "web_api.products.{name}.consumer {other:?} is not canonical"
            ));
            String::new()
        }
        // A redirect declares no consumer, and asking for one would be
        // asking for a vault identity nothing authenticates as.
        None if redirect_to.is_some() || upstream_service.is_some() => String::new(),
        None => {
            problems.push(format!("web_api.products.{name}.consumer is required"));
            String::new()
        }
    };
    let readyz = match entry.get("readyz") {
        Some(Value::String(path)) if path.starts_with('/') && !path.contains(' ') => path.clone(),
        Some(other) => {
            problems.push(format!(
                "web_api.products.{name}.readyz {other} must be an absolute request path"
            ));
            String::new()
        }
        None => "/".to_string(),
    };
    let edge = match entry.get("edge") {
        Some(Value::String(edge)) if WEB_API_EDGES.contains(&edge.as_str()) => edge.clone(),
        Some(other) => {
            problems.push(format!(
                "web_api.products.{name}.edge {other} is not one of {WEB_API_EDGES:?}"
            ));
            String::new()
        }
        None => "stado".to_string(),
    };
    let mut env = BTreeMap::new();
    match entry.get("env") {
        Some(Value::Object(values)) => {
            for (key, value) in values {
                match value.as_str() {
                    Some(value) if is_env_name(key) && !value.chars().any(char::is_control) => {
                        env.insert(key.clone(), value.to_string());
                    }
                    _ => problems.push(format!(
                        "web_api.products.{name}.env.{key} must be a plain string value"
                    )),
                }
            }
        }
        Some(_) => problems.push(format!("web_api.products.{name}.env must be an object")),
        None => {}
    }
    let mut secrets = BTreeMap::new();
    match entry.get("secrets") {
        Some(Value::Object(values)) => {
            for (key, value) in values {
                match value.as_str() {
                    Some(reference)
                        if is_env_name(key) && parse_secret_reference(reference).is_some() =>
                    {
                        secrets.insert(key.clone(), reference.to_string());
                    }
                    _ => problems.push(format!(
                        "web_api.products.{name}.secrets.{key} must be an \"item#field\" reference"
                    )),
                }
            }
        }
        Some(_) => problems.push(format!("web_api.products.{name}.secrets must be an object")),
        None => {}
    }
    let mut database = None;
    match entry.get("database") {
        Some(Value::Object(declared)) => {
            for key in declared.keys() {
                if !matches!(key.as_str(), "name" | "field" | "variable") {
                    problems.push(format!(
                        "web_api.products.{name}.database contains unsupported key {key:?}"
                    ));
                }
            }
            let declared_name = declared.get("name").and_then(Value::as_str).unwrap_or("");
            let field = declared.get("field").and_then(Value::as_str).unwrap_or("");
            let variable = declared
                .get("variable")
                .and_then(Value::as_str)
                .unwrap_or("");
            let field_name = !field.is_empty()
                && field.chars().all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
                });
            if !canonical_machine_name(declared_name) {
                problems.push(format!(
                    "web_api.products.{name}.database.name is required and must be canonical"
                ));
            } else if !field_name {
                problems.push(format!(
                    "web_api.products.{name}.database.field is required and must be a field name"
                ));
            } else if !is_env_name(variable) {
                problems.push(format!(
                    "web_api.products.{name}.database.variable is required and must be an environment name"
                ));
            } else {
                database = Some(WebApiDatabase {
                    name: declared_name.to_string(),
                    field: field.to_string(),
                    variable: variable.to_string(),
                });
            }
        }
        Some(_) => {
            problems.push(format!(
                "web_api.products.{name}.database must be an object with name, field and variable"
            ));
        }
        None => {}
    }
    WebApiUnit {
        consumer,
        readyz,
        edge,
        env,
        secrets,
        database,
    }
}
