//! The client policy document: who may consume, and in which namespaces.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use serde::Deserialize;
use serde_json::Value;

const CONSUME_ACTION: &str = "consume";

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawClient {
    consumer: String,
    item: String,
    namespaces: Vec<String>,
    actions: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct RateLimitClient {
    name: String,
    item: String,
    namespaces: BTreeSet<String>,
}

impl RateLimitClient {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn item(&self) -> &str {
        &self.item
    }

    pub fn allows_namespace(&self, namespace: &str) -> bool {
        self.namespaces.contains(namespace)
    }
}

fn canonical_name(value: &str) -> bool {
    !value.is_empty()
        && value.trim() == value
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

pub(crate) fn parse_clients(
    value: Option<Value>,
) -> Result<BTreeMap<String, RateLimitClient>, String> {
    let Some(Value::Object(entries)) = value else {
        return Err("rate_limit.clients must be a non-empty client mapping".to_string());
    };
    if entries.is_empty() {
        return Err("rate_limit.clients must not be empty".to_string());
    }

    let mut clients = BTreeMap::new();
    let mut consumers = BTreeSet::new();
    let mut items = BTreeSet::new();
    let mut namespaces = BTreeSet::new();
    for (name, value) in entries {
        if !canonical_name(&name) {
            return Err(format!("rate_limit.clients key {name:?} is not canonical"));
        }
        let raw: RawClient = serde_json::from_value(value)
            .map_err(|error| format!("invalid rate_limit.clients.{name}: {error}"))?;
        let expected_consumer = format!("{name}-rate-limit-client");
        let expected_item = format!("{name}-rate-limit-api");
        if raw.consumer != expected_consumer {
            return Err(format!(
                "rate_limit.clients.{name}.consumer must be {expected_consumer:?}"
            ));
        }
        if raw.item != expected_item {
            return Err(format!(
                "rate_limit.clients.{name}.item must be {expected_item:?}"
            ));
        }
        if !consumers.insert(raw.consumer.clone()) || !items.insert(raw.item.clone()) {
            return Err("rate_limit.clients must use distinct consumers and items".to_string());
        }
        if raw.actions.as_slice() != [CONSUME_ACTION] {
            return Err(format!(
                "rate_limit.clients.{name}.actions must contain only consume"
            ));
        }
        if raw.namespaces.is_empty() {
            return Err(format!(
                "rate_limit.clients.{name}.namespaces must not be empty"
            ));
        }
        let mut client_namespaces = BTreeSet::new();
        for namespace in raw.namespaces {
            if !canonical_name(&namespace) || !client_namespaces.insert(namespace.clone()) {
                return Err(format!(
                    "rate_limit.clients.{name}.namespaces contains a malformed or duplicate namespace"
                ));
            }
            if !namespaces.insert(namespace.clone()) {
                return Err(format!(
                    "rate-limit namespace {namespace:?} is assigned to more than one client"
                ));
            }
        }
        clients.insert(
            name.clone(),
            RateLimitClient {
                name,
                item: raw.item,
                namespaces: client_namespaces,
            },
        );
    }
    Ok(clients)
}

static CLIENTS: LazyLock<Result<BTreeMap<String, RateLimitClient>, String>> = LazyLock::new(|| {
    let configured = match std::env::var("WC_RATE_LIMIT_CLIENTS")
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        Some(encoded) => match serde_json::from_str::<Value>(&encoded) {
            Ok(value) => Some(value),
            Err(error) => return Err(format!("WC_RATE_LIMIT_CLIENTS must be JSON: {error}")),
        },
        None => crate::config_file::get("rate_limit.clients"),
    };
    parse_clients(configured)
});

pub fn clients() -> Result<&'static BTreeMap<String, RateLimitClient>, &'static str> {
    CLIENTS.as_ref().map_err(String::as_str)
}
