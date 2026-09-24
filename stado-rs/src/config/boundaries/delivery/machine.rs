//! Machine boundary: submit/status/cancel clients. Their bearers are read as
//! Stado's Skarbiec identity `stado`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use crate::config::canonical_machine_name;
use serde_json::Value;

/// What the machine API lets a client do, from the boundaries' declaration.
pub fn machine_api_actions() -> Vec<String> {
    super::super::declared_actions("machine")
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MachineApiClient {
    item: String,
    actions: Vec<String>,
    targets: Vec<String>,
}

impl MachineApiClient {
    pub fn item(&self) -> &str {
        &self.item
    }

    pub fn allows_action(&self, action: &str) -> bool {
        self.actions.iter().any(|allowed| allowed == action)
    }

    pub fn allows_target(&self, target: &str) -> bool {
        self.targets.iter().any(|allowed| allowed == target)
    }

    pub fn targets(&self) -> &[String] {
        &self.targets
    }
}

pub(crate) fn parse_machine_api_clients(
    value: Option<&Value>,
) -> Result<BTreeMap<String, MachineApiClient>, Vec<String>> {
    let Some(Value::Object(entries)) = value else {
        return Err(vec![
            "machine_api.clients must be a non-empty exact client mapping".to_string(),
        ]);
    };
    if entries.is_empty() {
        return Err(vec!["machine_api.clients must not be empty".to_string()]);
    }
    let mut problems = Vec::new();
    let mut clients = BTreeMap::new();
    let mut items = BTreeSet::new();
    for (name, raw) in entries {
        let start = problems.len();
        if !canonical_machine_name(name) {
            problems.push(format!("machine_api.clients key {name:?} is not canonical"));
        }
        let Some(entry) = raw.as_object() else {
            problems.push(format!("machine_api.clients.{name} must be an object"));
            continue;
        };
        for key in entry.keys() {
            if !matches!(key.as_str(), "item" | "actions" | "targets") {
                problems.push(format!(
                    "machine_api.clients.{name} contains unsupported key {key:?}"
                ));
            }
        }
        let item = entry
            .get("item")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let expected_item = format!("{name}-machine-api");
        if item != expected_item {
            problems.push(format!(
                "machine_api.clients.{name}.item must be {expected_item:?}"
            ));
        }
        if !items.insert(item.to_string()) {
            problems.push(format!(
                "machine_api.clients maps more than one client to item {item:?}"
            ));
        }
        let mut actions = Vec::new();
        match entry.get("actions") {
            Some(Value::Array(values)) if !values.is_empty() => {
                let mut seen = BTreeSet::new();
                for value in values {
                    let Some(action) = value.as_str() else {
                        problems.push(format!(
                            "machine_api.clients.{name}.actions entries must be strings"
                        ));
                        continue;
                    };
                    if !machine_api_actions().iter().any(|known| known == action)
                        || !seen.insert(action)
                    {
                        problems.push(format!(
                            "machine_api.clients.{name}.actions contains unsupported or duplicate {action:?}"
                        ));
                        continue;
                    }
                    actions.push(action.to_string());
                }
            }
            _ => problems.push(format!(
                "machine_api.clients.{name}.actions must be a non-empty array"
            )),
        }
        let mut targets = Vec::new();
        match entry.get("targets") {
            Some(Value::Array(values)) if !values.is_empty() => {
                let mut seen = BTreeSet::new();
                for value in values {
                    let Some(target) = value.as_str() else {
                        problems.push(format!(
                            "machine_api.clients.{name}.targets entries must be strings"
                        ));
                        continue;
                    };
                    let known = crate::capabilities::configurable_variant(
                        crate::capabilities::RuntimeFacet::Compute,
                        target,
                    )
                    .is_some();
                    if !known || !seen.insert(target) {
                        problems.push(format!(
                            "machine_api.clients.{name}.targets contains unknown or duplicate {target:?}"
                        ));
                        continue;
                    }
                    targets.push(target.to_string());
                }
            }
            _ => problems.push(format!(
                "machine_api.clients.{name}.targets must be a non-empty array"
            )),
        }
        if problems.len() == start {
            clients.insert(
                name.to_string(),
                MachineApiClient {
                    item: item.to_string(),
                    actions,
                    targets,
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

static MACHINE_API_CLIENTS: LazyLock<Result<BTreeMap<String, MachineApiClient>, Vec<String>>> =
    LazyLock::new(|| {
        let configured = match std::env::var("WC_MACHINE_API_CLIENTS")
            .ok()
            .filter(|value| !value.trim().is_empty())
        {
            Some(encoded) => match serde_json::from_str::<Value>(&encoded) {
                Ok(value) => Some(value),
                Err(error) => {
                    return Err(vec![format!(
                        "WC_MACHINE_API_CLIENTS must be a JSON object: {error}"
                    )])
                }
            },
            None => crate::config_file::get("machine_api.clients"),
        };
        parse_machine_api_clients(configured.as_ref())
    });

pub fn machine_api_clients(
) -> Result<&'static BTreeMap<String, MachineApiClient>, &'static [String]> {
    match &*MACHINE_API_CLIENTS {
        Ok(clients) => Ok(clients),
        Err(problems) => Err(problems.as_slice()),
    }
}
