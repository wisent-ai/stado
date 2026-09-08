//! Registry-policy boundary: clients, actions and its Skarbiec grant.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use crate::config::skarbiec_url;
use crate::config_file::{expand_tilde, resolve as cfg};
use serde_json::Value;

pub const REGISTRY_API_VERIFIER_CONSUMER: &str = "stado-registry-api-verifier";
/// Actions a registry-API client may be granted.
///
/// `policy-read`, `cleanup-read`, and `converge-read` answer questions;
/// `policy-write` rewrites one target's whitelisted policy fields,
/// `cleanup-run` asks the local janitor for a pass, `registry-import` additively
/// adopts a complete registry-v2 document, and `converge-apply` may deliver every
/// declared binary on one canonical host. Reading does not authorize mutation;
/// each independent operation requires its own grant.
/// Storage reconciliation separately grants status reads and explicit transaction
/// actions; neither policy writes nor software delivery authorize a data handoff.
pub const REGISTRY_API_ACTIONS: &[&str] = &[
    "cleanup-read",
    "cleanup-run",
    "converge-apply",
    "converge-read",
    "policy-read",
    "policy-write",
    "registry-import",
    "storage-reconcile-apply",
    "storage-reconcile-read",
];

/// One client authorized against the registry-policy boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegistryApiClient {
    item: String,
    actions: Vec<String>,
}

impl RegistryApiClient {
    pub fn item(&self) -> &str {
        &self.item
    }

    pub fn allows_action(&self, action: &str) -> bool {
        self.actions.iter().any(|allowed| allowed == action)
    }
}

/// Parse `registry_api.clients`.
///
/// An ABSENT or empty mapping is `Ok(empty)`, not an error, and that is the
/// whole judgement in this function. Every route on this boundary then
/// refuses with `401`, because nobody has been granted it — which is a
/// refusal, not an outage. Returning `Err` here would make an undeclared
/// boundary answer `503 authorization unavailable` and send an operator to
/// look for a broken vault instead of an absent declaration; this file
/// already learned that distinction where the janitor reads a policy, and it
/// is the same distinction.
///
/// A mapping that EXISTS and cannot be read is still an error: a declaration
/// nobody can parse is a configuration fault, and 503 is the honest answer.
pub(crate) fn parse_registry_api_clients(
    value: Option<&Value>,
) -> Result<BTreeMap<String, RegistryApiClient>, Vec<String>> {
    let mut clients = BTreeMap::new();
    let entries = match value {
        None => return Ok(clients),
        Some(Value::Object(entries)) if entries.is_empty() => return Ok(clients),
        Some(Value::Object(entries)) => entries,
        Some(_) => {
            return Err(vec![
                "registry_api.clients must be an exact client mapping".to_string()
            ])
        }
    };
    let mut problems = Vec::new();
    let mut items = BTreeSet::new();
    for (name, raw) in entries {
        let Some(entry) = raw.as_object() else {
            problems.push(format!("registry_api.clients.{name} must be an object"));
            continue;
        };
        for key in entry.keys() {
            if !matches!(key.as_str(), "item" | "actions") {
                problems.push(format!(
                    "registry_api.clients.{name} contains unsupported key {key:?}"
                ));
            }
        }
        // The item name is derived, not chosen, so a client cannot be pointed
        // at another boundary's credential by editing one string.
        let expected_item = format!("{name}-registry-api");
        let item = entry
            .get("item")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if item != expected_item {
            problems.push(format!(
                "registry_api.clients.{name}.item must be {expected_item:?}"
            ));
        }
        if !items.insert(item.clone()) {
            problems.push(format!(
                "registry_api.clients maps more than one client to item {item:?}"
            ));
        }
        let mut actions = Vec::new();
        match entry.get("actions") {
            Some(Value::Array(values)) if !values.is_empty() => {
                let mut seen = BTreeSet::new();
                for value in values {
                    let Some(action) = value.as_str() else {
                        problems.push(format!(
                            "registry_api.clients.{name}.actions entries must be strings"
                        ));
                        continue;
                    };
                    if !REGISTRY_API_ACTIONS.contains(&action) {
                        problems.push(format!(
                            "registry_api.clients.{name}.actions contains unknown action {action:?}"
                        ));
                        continue;
                    }
                    if !seen.insert(action) {
                        problems.push(format!(
                            "registry_api.clients.{name}.actions repeats {action:?}"
                        ));
                        continue;
                    }
                    actions.push(action.to_string());
                }
            }
            _ => problems.push(format!(
                "registry_api.clients.{name}.actions must be a non-empty array"
            )),
        }
        actions.sort();
        clients.insert(name.clone(), RegistryApiClient { item, actions });
    }
    if problems.is_empty() {
        Ok(clients)
    } else {
        Err(problems)
    }
}

static REGISTRY_API_CLIENTS: LazyLock<Result<BTreeMap<String, RegistryApiClient>, Vec<String>>> =
    LazyLock::new(|| {
        let configured = match std::env::var("WC_REGISTRY_API_CLIENTS")
            .ok()
            .filter(|value| !value.trim().is_empty())
        {
            Some(encoded) => match serde_json::from_str::<Value>(&encoded) {
                Ok(value) => Some(value),
                Err(error) => {
                    return Err(vec![format!(
                        "WC_REGISTRY_API_CLIENTS must be a JSON object: {error}"
                    )])
                }
            },
            None => crate::config_file::get("registry_api.clients"),
        };
        parse_registry_api_clients(configured.as_ref())
    });
static REGISTRY_SKARBIEC_URL: LazyLock<String> = LazyLock::new(|| {
    cfg(
        "WC_REGISTRY_SKARBIEC_URL",
        "registry_api.skarbiec.url",
        skarbiec_url(),
    )
});
static REGISTRY_SKARBIEC_CONSUMER: LazyLock<String> = LazyLock::new(|| {
    cfg(
        "WC_REGISTRY_SKARBIEC_CONSUMER",
        "registry_api.skarbiec.consumer",
        REGISTRY_API_VERIFIER_CONSUMER,
    )
});
static REGISTRY_SKARBIEC_TOKEN_FILE: LazyLock<String> = LazyLock::new(|| {
    let default = std::env::var("HOME")
        .map(|home| {
            std::path::Path::new(&home)
                .join(".stado")
                .join("stado-registry-api-verifier-skarbiec-token")
                .to_string_lossy()
                .into_owned()
        })
        .unwrap_or_default();
    expand_tilde(&cfg(
        "WC_REGISTRY_SKARBIEC_TOKEN_FILE",
        "registry_api.skarbiec.token_file",
        &default,
    ))
    .to_string_lossy()
    .into_owned()
});

pub fn registry_api_clients(
) -> Result<&'static BTreeMap<String, RegistryApiClient>, &'static [String]> {
    match &*REGISTRY_API_CLIENTS {
        Ok(clients) => Ok(clients),
        Err(problems) => Err(problems.as_slice()),
    }
}

pub fn registry_skarbiec_url() -> &'static str {
    REGISTRY_SKARBIEC_URL.as_str()
}

pub fn registry_skarbiec_consumer() -> &'static str {
    REGISTRY_SKARBIEC_CONSUMER.as_str()
}

pub fn registry_skarbiec_token_file() -> &'static str {
    REGISTRY_SKARBIEC_TOKEN_FILE.as_str()
}
