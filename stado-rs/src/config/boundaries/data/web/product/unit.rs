//! Consumer, ready path, edge, environment and database of one web product.

use std::collections::BTreeMap;

use super::WebApiDatabase;
use crate::config::{canonical_machine_name, is_env_name, parse_secret_reference, WebEdge};
use serde_json::{Map, Value};

/// The unit half of one declaration: the identity it runs as, its ready path,
/// its edge, and the environment its unit is delivered with.
pub(super) struct WebApiUnit {
    pub(super) consumer: String,
    pub(super) readyz: String,
    pub(super) edge: WebEdge,
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
        None => WebEdge::default(),
        Some(raw) => match <WebEdge as serde::Deserialize>::deserialize(raw) {
            Ok(edge) => edge,
            Err(error) => {
                problems.push(format!("web_api.products.{name}.edge {raw}: {error}"));
                WebEdge::default()
            }
        },
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
        Some(raw_database @ Value::Object(declared)) => {
            #[derive(serde::Deserialize)]
            #[serde(deny_unknown_fields)]
            #[allow(dead_code)]
            struct Fields {
                name: Option<serde::de::IgnoredAny>,
                field: Option<serde::de::IgnoredAny>,
                variable: Option<serde::de::IgnoredAny>,
            }
            crate::config::boundaries::declared_fields::<Fields>(
                raw_database,
                &format!("web_api.products.{name}.database"),
                problems,
            );
            // Each field is read as itself: absent, not a string, and a
            // string of the wrong shape are three problems with three
            // sentences, so the repair is the one the cause needs.
            let location = format!("web_api.products.{name}.database");
            let read = |key: &str, problems: &mut Vec<String>| -> Option<String> {
                match declared.get(key) {
                    Some(Value::String(text)) => Some(text.clone()),
                    Some(other) => {
                        problems.push(format!("{location}.{key} must be a string, not {other}"));
                        None
                    }
                    None => {
                        problems.push(format!("{location}.{key} is required"));
                        None
                    }
                }
            };
            let declared_name = read("name", problems).filter(|value| {
                let canonical = canonical_machine_name(value);
                if !canonical {
                    problems.push(format!("{location}.name must be canonical"));
                }
                canonical
            });
            let field = read("field", problems).filter(|value| {
                let field_name = !value.is_empty()
                    && value.chars().all(|character| {
                        character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
                    });
                if !field_name {
                    problems.push(format!("{location}.field must be a field name"));
                }
                field_name
            });
            let variable = read("variable", problems).filter(|value| {
                let env_name = is_env_name(value);
                if !env_name {
                    problems.push(format!("{location}.variable must be an environment name"));
                }
                env_name
            });
            if let (Some(name), Some(field), Some(variable)) = (declared_name, field, variable) {
                database = Some(WebApiDatabase {
                    name,
                    field,
                    variable,
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
