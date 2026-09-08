//! Declared fleet databases: engine, scopes, credential item and consumers.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use crate::config::canonical_machine_name;
use serde_json::Value;

pub const DATABASE_API_ENGINES: &[&str] = &["postgres", "sqlite"];
pub const DATABASE_API_SCOPES: &[&str] = &["read", "write"];

/// One declared fleet database: where its credential lives, what engine
/// speaks it, which scopes exist, and which consumers may resolve it.
///
/// The plane deliberately holds the coordinate, not the secret. A consumer
/// that resolves a database learns the endpoint and the Skarbiec item to
/// acquire; the value behind the item never passes through this surface.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DatabaseApiDatabase {
    item: String,
    engine: String,
    scopes: Vec<String>,
    consumers: Vec<String>,
}

impl DatabaseApiDatabase {
    pub fn item(&self) -> &str {
        &self.item
    }

    pub fn engine(&self) -> &str {
        &self.engine
    }

    pub fn scopes(&self) -> &[String] {
        &self.scopes
    }

    pub fn consumers(&self) -> &[String] {
        &self.consumers
    }

    pub fn allows_consumer(&self, consumer: &str) -> bool {
        self.consumers.iter().any(|allowed| allowed == consumer)
    }
}

pub(crate) fn parse_database_api_databases(
    value: Option<&Value>,
) -> Result<BTreeMap<String, DatabaseApiDatabase>, Vec<String>> {
    let Some(Value::Object(entries)) = value else {
        return Err(vec![
            "database_api.databases must be a non-empty object mapping database names to declarations"
                .to_string(),
        ]);
    };
    if entries.is_empty() {
        return Err(vec!["database_api.databases must not be empty".to_string()]);
    }
    let mut problems = Vec::new();
    let mut databases = BTreeMap::new();
    let mut items = BTreeSet::new();
    for (name, raw) in entries {
        let start = problems.len();
        if !canonical_machine_name(name) {
            problems.push(format!(
                "database_api.databases key {name:?} is not canonical"
            ));
        }
        let Some(entry) = raw.as_object() else {
            problems.push(format!(
                "database_api.databases.{name} must be an object with item, engine, scopes and consumers"
            ));
            continue;
        };
        for key in entry.keys() {
            if !matches!(key.as_str(), "item" | "engine" | "scopes" | "consumers") {
                problems.push(format!(
                    "database_api.databases.{name} contains unsupported key {key:?}"
                ));
            }
        }
        let expected_item = format!("{name}-database");
        let item = entry
            .get("item")
            .and_then(Value::as_str)
            .unwrap_or(&expected_item);
        if item != expected_item {
            problems.push(format!(
                "database_api.databases.{name}.item must be {expected_item:?}, got {item:?}"
            ));
        }
        if !items.insert(item.to_string()) {
            problems.push(format!(
                "database_api.databases maps more than one database to item {item:?}"
            ));
        }
        let engine = match entry.get("engine").and_then(Value::as_str) {
            Some(engine) if DATABASE_API_ENGINES.contains(&engine) => engine.to_string(),
            Some(other) => {
                problems.push(format!(
                    "database_api.databases.{name}.engine {other:?} is not one of {DATABASE_API_ENGINES:?}"
                ));
                String::new()
            }
            None => {
                problems.push(format!("database_api.databases.{name}.engine is required"));
                String::new()
            }
        };
        let mut scopes = Vec::new();
        match entry.get("scopes") {
            Some(Value::Array(raw_scopes)) if !raw_scopes.is_empty() => {
                let mut seen = BTreeSet::new();
                for scope in raw_scopes {
                    match scope.as_str() {
                        Some(scope) if DATABASE_API_SCOPES.contains(&scope) && seen.insert(scope) => {
                            scopes.push(scope.to_string());
                        }
                        Some(other) => problems.push(format!(
                            "database_api.databases.{name}.scopes contains unsupported or duplicate {other:?}"
                        )),
                        None => problems.push(format!(
                            "database_api.databases.{name}.scopes entries must be strings"
                        )),
                    }
                }
            }
            Some(Value::Array(_)) => problems.push(format!(
                "database_api.databases.{name}.scopes must not be empty"
            )),
            Some(_) => problems.push(format!(
                "database_api.databases.{name}.scopes must be an array"
            )),
            None => problems.push(format!("database_api.databases.{name}.scopes is required")),
        }
        let mut consumers = Vec::new();
        match entry.get("consumers") {
            Some(Value::Array(raw_consumers)) if !raw_consumers.is_empty() => {
                let mut seen = BTreeSet::new();
                for consumer in raw_consumers {
                    match consumer.as_str() {
                        Some(consumer) if canonical_machine_name(consumer) && seen.insert(consumer) => {
                            consumers.push(consumer.to_string());
                        }
                        Some(other) => problems.push(format!(
                            "database_api.databases.{name}.consumers contains non-canonical or duplicate {other:?}"
                        )),
                        None => problems.push(format!(
                            "database_api.databases.{name}.consumers entries must be strings"
                        )),
                    }
                }
            }
            Some(Value::Array(_)) => problems.push(format!(
                "database_api.databases.{name}.consumers must not be empty"
            )),
            Some(_) => problems.push(format!(
                "database_api.databases.{name}.consumers must be an array"
            )),
            None => problems.push(format!(
                "database_api.databases.{name}.consumers is required"
            )),
        }
        if problems.len() == start {
            databases.insert(
                name.clone(),
                DatabaseApiDatabase {
                    item: item.to_string(),
                    engine,
                    scopes,
                    consumers,
                },
            );
        }
    }
    if problems.is_empty() {
        Ok(databases)
    } else {
        Err(problems)
    }
}

static DATABASE_API_DATABASES: LazyLock<
    Result<BTreeMap<String, DatabaseApiDatabase>, Vec<String>>,
> = LazyLock::new(|| {
    let configured = match std::env::var("WC_DATABASE_API_DATABASES")
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        Some(encoded) => serde_json::from_str::<Value>(&encoded)
            .map_err(|error| {
                vec![format!(
                    "WC_DATABASE_API_DATABASES must be valid JSON: {error}"
                )]
            })
            .and_then(|parsed| parse_database_api_databases(Some(&parsed))),
        None => {
            parse_database_api_databases(crate::config_file::get("database_api.databases").as_ref())
        }
    };
    configured
});

pub fn database_api_databases(
) -> Result<&'static BTreeMap<String, DatabaseApiDatabase>, &'static [String]> {
    match &*DATABASE_API_DATABASES {
        Ok(databases) => Ok(databases),
        Err(problems) => Err(problems.as_slice()),
    }
}
