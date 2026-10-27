//! Integration clients: the domains a client may reach.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use super::{canonical_integration_component, INTEGRATION_CLIENT_DOMAINS};
use serde_json::Value;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IntegrationClient {
    item: String,
    allowed_actions: Vec<String>,
}

impl IntegrationClient {
    pub fn item(&self) -> &str {
        &self.item
    }

    pub fn allows(&self, domain: &str, action: &str) -> bool {
        self.allowed_actions.iter().any(|allowed| {
            allowed
                .split_once('/')
                .is_some_and(|(allowed_domain, allowed_action)| {
                    allowed_domain == domain && allowed_action == action
                })
        })
    }

    pub fn allowed_actions(&self) -> &[String] {
        &self.allowed_actions
    }
}

pub(crate) fn parse_integration_clients(
    value: Option<&Value>,
) -> Result<BTreeMap<String, IntegrationClient>, Vec<String>> {
    let Some(Value::Object(entries)) = value else {
        return Err(vec![
            "integration.clients must be a non-empty exact client mapping".to_string(),
        ]);
    };
    if entries.is_empty() {
        return Err(vec!["integration.clients must not be empty".to_string()]);
    }
    let mut problems = Vec::new();
    let mut clients = BTreeMap::new();
    let mut items = BTreeSet::new();
    for (name, raw) in entries {
        let start = problems.len();
        if !canonical_integration_component(name) || name.contains('.') {
            problems.push(format!("integration.clients key {name:?} is not canonical"));
        }
        let Some(entry) = raw.as_object() else {
            problems.push(format!("integration.clients.{name} must be an object"));
            continue;
        };
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        #[allow(dead_code)]
        struct Fields {
            item: Option<serde::de::IgnoredAny>,
            allowed_actions: Option<serde::de::IgnoredAny>,
        }
        crate::config::boundaries::declared_fields::<Fields>(
            raw,
            &format!("integration.clients.{name}"),
            &mut problems,
        );
        // A client is named by the Skarbiec item it presents; a declaration
        // without one is refused by name, and the checks below run on the
        // item it does declare.
        let item = match entry.get("item").and_then(Value::as_str) {
            Some(item) => item,
            None => {
                problems.push(format!(
                    "integration.clients.{name}.item must name a canonical product-specific \
                     *-integration-api item, and is absent"
                ));
                ""
            }
        };
        if !item.is_empty()
            && (!canonical_integration_component(item) || !item.ends_with("-integration-api"))
        {
            problems.push(format!(
                "integration.clients.{name}.item must be a canonical product-specific *-integration-api item"
            ));
        }
        if !item.is_empty() && !items.insert(item.to_string()) {
            problems.push(format!(
                "integration.clients maps more than one client to item {item:?}"
            ));
        }
        let mut allowed_actions = Vec::new();
        match entry.get("allowed_actions") {
            Some(Value::Array(values)) if !values.is_empty() => {
                let mut seen = BTreeSet::new();
                for value in values {
                    let Some(value) = value.as_str() else {
                        problems.push(format!(
                            "integration.clients.{name}.allowed_actions entries must be strings"
                        ));
                        continue;
                    };
                    let canonical = value.split_once('/').is_some_and(|(domain, action)| {
                        !domain.contains('.')
                            && canonical_integration_component(domain)
                            && INTEGRATION_CLIENT_DOMAINS.contains(&domain)
                            && canonical_integration_component(action)
                    });
                    if !canonical || !seen.insert(value) {
                        problems.push(format!(
                            "integration.clients.{name}.allowed_actions contains invalid or duplicate {value:?}"
                        ));
                        continue;
                    }
                    allowed_actions.push(value.to_string());
                }
            }
            _ => problems.push(format!(
                "integration.clients.{name}.allowed_actions must be a non-empty array"
            )),
        }
        if problems.len() == start {
            clients.insert(
                name.to_string(),
                IntegrationClient {
                    item: item.to_string(),
                    allowed_actions,
                },
            );
        }
    }
    if problems.is_empty() {
        Ok(clients)
    } else {
        Err(problems)
    }
}

static INTEGRATION_CLIENTS: LazyLock<Result<BTreeMap<String, IntegrationClient>, Vec<String>>> =
    LazyLock::new(|| {
        // The environment declaration wins over the configuration file;
        // unset or blank, the file's `integration.clients` is read. A value
        // that is not Unicode cannot be JSON and is refused as such.
        let configured = match std::env::var("WC_INTEGRATION_CLIENTS") {
            Ok(encoded) if !encoded.trim().is_empty() => {
                match serde_json::from_str::<Value>(&encoded) {
                    Ok(value) => Some(value),
                    Err(error) => {
                        return Err(vec![format!(
                            "WC_INTEGRATION_CLIENTS must be a JSON object: {error}"
                        )])
                    }
                }
            }
            Ok(_) | Err(std::env::VarError::NotPresent) => {
                crate::config_file::get("integration.clients")
            }
            Err(std::env::VarError::NotUnicode(raw)) => {
                return Err(vec![format!(
                    "WC_INTEGRATION_CLIENTS must be a JSON object, and is not Unicode: {raw:?}"
                )])
            }
        };
        parse_integration_clients(configured.as_ref())
    });

pub fn integration_clients(
) -> Result<&'static BTreeMap<String, IntegrationClient>, &'static [String]> {
    match &*INTEGRATION_CLIENTS {
        Ok(clients) => Ok(clients),
        Err(problems) => Err(problems.as_slice()),
    }
}
