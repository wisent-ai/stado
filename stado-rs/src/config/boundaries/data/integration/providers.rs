//! Integration providers: the provider grants Stado itself resolves.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use super::{canonical_integration_component, INTEGRATION_PROVIDER_DOMAINS};
use crate::config_file::expand_tilde;
use serde_json::Value;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IntegrationProvider {
    consumer: String,
    token_file: String,
    items: Vec<String>,
}

impl IntegrationProvider {
    pub fn consumer(&self) -> &str {
        &self.consumer
    }

    pub fn token_file(&self) -> &str {
        &self.token_file
    }

    pub fn items(&self) -> &[String] {
        &self.items
    }
}

pub(crate) fn parse_integration_providers(
    value: Option<&Value>,
) -> Result<BTreeMap<String, IntegrationProvider>, Vec<String>> {
    let entries = match value {
        None => return Ok(BTreeMap::new()),
        Some(Value::Object(entries)) => entries,
        Some(_) => {
            return Err(vec![
                "integration.providers must be an exact domain mapping".to_string(),
            ])
        }
    };
    if entries.is_empty() {
        return Ok(BTreeMap::new());
    }
    let mut problems = Vec::new();
    let mut providers = BTreeMap::new();
    let mut consumers = BTreeSet::new();
    let mut token_files = BTreeSet::new();
    let mut all_items = BTreeSet::new();
    for (domain, raw) in entries {
        let start = problems.len();
        if !canonical_integration_component(domain) || domain.contains('.') {
            problems.push(format!(
                "integration.providers key {domain:?} is not canonical"
            ));
        }
        if !INTEGRATION_PROVIDER_DOMAINS.contains(&domain.as_str()) {
            problems.push(format!(
                "integration.providers contains unsupported domain {domain:?}"
            ));
        }
        let Some(entry) = raw.as_object() else {
            problems.push(format!("integration.providers.{domain} must be an object"));
            continue;
        };
        for key in entry.keys() {
            if !matches!(key.as_str(), "consumer" | "token_file" | "items") {
                problems.push(format!(
                    "integration.providers.{domain} contains unsupported key {key:?}"
                ));
            }
        }
        let consumer = entry
            .get("consumer")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let expected_consumer = format!("stado-{domain}-integration-provider");
        if consumer != expected_consumer {
            problems.push(format!(
                "integration.providers.{domain}.consumer must be {expected_consumer:?}"
            ));
        }
        if !consumers.insert(consumer.to_string()) {
            problems.push(format!(
                "integration.providers reuses consumer {consumer:?}"
            ));
        }
        let token_file = entry
            .get("token_file")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if token_file.trim().is_empty() {
            problems.push(format!(
                "integration.providers.{domain}.token_file is required"
            ));
        }
        let token_file = expand_tilde(token_file).to_string_lossy().into_owned();
        if !token_files.insert(token_file.clone()) {
            problems.push(format!(
                "integration.providers reuses token_file for domain {domain:?}"
            ));
        }
        let mut items = Vec::new();
        match entry.get("items") {
            Some(Value::Array(values)) if !values.is_empty() => {
                let mut seen = BTreeSet::new();
                for value in values {
                    let Some(item) = value.as_str() else {
                        problems.push(format!(
                            "integration.providers.{domain}.items entries must be strings"
                        ));
                        continue;
                    };
                    if !canonical_integration_component(item)
                        || item.ends_with("-integration-api")
                        || !seen.insert(item)
                        || !all_items.insert(item)
                    {
                        problems.push(format!(
                            "integration.providers.{domain}.items contains invalid, duplicate, or cross-domain item {item:?}"
                        ));
                        continue;
                    }
                    items.push(item.to_string());
                }
            }
            _ => problems.push(format!(
                "integration.providers.{domain}.items must be a non-empty array"
            )),
        }
        if problems.len() == start {
            providers.insert(
                domain.to_string(),
                IntegrationProvider {
                    consumer: consumer.to_string(),
                    token_file,
                    items,
                },
            );
        }
    }
    if problems.is_empty() {
        Ok(providers)
    } else {
        Err(problems)
    }
}

static INTEGRATION_PROVIDERS: LazyLock<Result<BTreeMap<String, IntegrationProvider>, Vec<String>>> =
    LazyLock::new(|| {
        let configured = match std::env::var("WC_INTEGRATION_PROVIDERS")
            .ok()
            .filter(|value| !value.trim().is_empty())
        {
            Some(encoded) => match serde_json::from_str::<Value>(&encoded) {
                Ok(value) => Some(value),
                Err(error) => {
                    return Err(vec![format!(
                        "WC_INTEGRATION_PROVIDERS must be a JSON object: {error}"
                    )])
                }
            },
            None => crate::config_file::get("integration.providers"),
        };
        parse_integration_providers(configured.as_ref())
    });

pub fn integration_providers(
) -> Result<&'static BTreeMap<String, IntegrationProvider>, &'static [String]> {
    match &*INTEGRATION_PROVIDERS {
        Ok(providers) => Ok(providers),
        Err(problems) => Err(problems.as_slice()),
    }
}

pub fn integration_provider(domain: &str) -> Option<&'static IntegrationProvider> {
    integration_providers().ok()?.get(domain)
}
