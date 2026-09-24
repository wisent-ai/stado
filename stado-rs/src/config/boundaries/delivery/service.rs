//! Service boundary: deployers and their actions.
//!
//! A product deploys as itself: its bearer is the vault item named after
//! the product, read through Stado's Skarbiec identity.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use serde_json::Value;

/// What the service API lets a deployer do, from the declaration.
pub fn service_api_actions() -> Vec<String> {
    super::super::declared_actions("service")
}
pub const ACTIVE_DEPLOYED_SERVICES: &[&str] = &["com.wisent.weles-api", "image-video-router"];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceDeployer {
    item: String,
    services: Vec<String>,
    actions: Vec<String>,
}

impl ServiceDeployer {
    pub fn item(&self) -> &str {
        &self.item
    }

    pub fn services(&self) -> &[String] {
        &self.services
    }

    pub fn actions(&self) -> &[String] {
        &self.actions
    }

    pub fn allows(&self, service: &str, action: &str) -> bool {
        self.services.iter().any(|configured| configured == service)
            && self.actions.iter().any(|configured| configured == action)
    }
}

pub(crate) fn parse_service_deployers(
    value: Option<&Value>,
) -> Result<BTreeMap<String, ServiceDeployer>, Vec<String>> {
    let Some(Value::Object(entries)) = value else {
        return Err(vec![
            "service_api.deployers must be a non-empty product-to-deployer mapping".to_string(),
        ]);
    };
    if entries.is_empty() {
        return Err(vec![
            "service_api.deployers must not be empty; managed-service routes fail closed"
                .to_string(),
        ]);
    }

    let canonical = |value: &str| {
        !value.is_empty()
            && value.trim() == value
            && value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    };
    let mut problems = Vec::new();
    let mut deployers = BTreeMap::new();
    let mut items = BTreeSet::new();
    let mut services_seen = BTreeSet::new();
    for (product, raw_entry) in entries {
        let mut entry_valid = true;
        if !canonical(product) {
            problems.push(format!(
                "service_api.deployers key {product:?} is not a canonical product name"
            ));
            entry_valid = false;
        }
        let Some(entry) = raw_entry.as_object() else {
            problems.push(format!(
                "service_api.deployers.{product} must contain item, services, and actions"
            ));
            continue;
        };
        for key in entry.keys() {
            match key.as_str() {
                "item" | "services" | "actions" => {}
                "consumer" => {
                    problems.push(format!(
                        "service_api.deployers.{product}.consumer is retired; configure \
                         Stado's secrets.skarbiec identity instead"
                    ));
                    entry_valid = false;
                }
                _ => {
                    problems.push(format!(
                        "service_api.deployers.{product} contains unsupported key {key:?}"
                    ));
                    entry_valid = false;
                }
            }
        }
        let item = entry
            .get("item")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if item != product.as_str() {
            problems.push(format!(
                "service_api.deployers.{product}.item must be the product's own name \
                 {product:?}, not {item:?}"
            ));
            entry_valid = false;
        }
        if !items.insert(item.to_string()) {
            problems.push(format!(
                "service_api.deployers maps more than one product to item {item:?}"
            ));
            entry_valid = false;
        }
        let services = match entry.get("services") {
            Some(Value::Array(values)) if !values.is_empty() => {
                let mut parsed = Vec::with_capacity(values.len());
                for value in values {
                    let Some(service) = value.as_str() else {
                        problems.push(format!(
                            "service_api.deployers.{product}.services entries must be strings"
                        ));
                        entry_valid = false;
                        continue;
                    };
                    if !canonical(service) && service != "com.wisent.weles-api" {
                        problems.push(format!(
                            "service_api.deployers.{product}.services contains non-canonical {service:?}"
                        ));
                        entry_valid = false;
                    }
                    if !services_seen.insert(service.to_string()) {
                        problems.push(format!(
                            "service {service:?} is mapped to more than one deployer"
                        ));
                        entry_valid = false;
                    }
                    parsed.push(service.to_string());
                }
                parsed
            }
            _ => {
                problems.push(format!(
                    "service_api.deployers.{product}.services must be a non-empty string array"
                ));
                entry_valid = false;
                Vec::new()
            }
        };
        let actions = match entry.get("actions") {
            Some(Value::Array(values)) if !values.is_empty() => {
                let mut parsed = Vec::with_capacity(values.len());
                let mut seen = BTreeSet::new();
                for value in values {
                    let Some(action) = value.as_str() else {
                        problems.push(format!(
                            "service_api.deployers.{product}.actions entries must be strings"
                        ));
                        entry_valid = false;
                        continue;
                    };
                    if !service_api_actions().iter().any(|known| known == action)
                        || !seen.insert(action.to_string())
                    {
                        problems.push(format!(
                            "service_api.deployers.{product}.actions contains unsupported or duplicate {action:?}"
                        ));
                        entry_valid = false;
                    }
                    parsed.push(action.to_string());
                }
                parsed
            }
            _ => {
                problems.push(format!(
                    "service_api.deployers.{product}.actions must be a non-empty string array"
                ));
                entry_valid = false;
                Vec::new()
            }
        };
        if entry_valid {
            deployers.insert(
                product.to_string(),
                ServiceDeployer {
                    item: item.to_string(),
                    services,
                    actions,
                },
            );
        }
    }
    for &required in ACTIVE_DEPLOYED_SERVICES {
        if !services_seen.contains(required) {
            problems.push(format!(
                "service_api.deployers is missing active service {required:?}"
            ));
        }
    }
    if problems.is_empty() {
        Ok(deployers)
    } else {
        Err(problems)
    }
}

static SERVICE_API_DEPLOYERS: LazyLock<Result<BTreeMap<String, ServiceDeployer>, Vec<String>>> =
    LazyLock::new(|| {
        let configured = match std::env::var("WC_SERVICE_API_DEPLOYERS")
            .ok()
            .filter(|value| !value.trim().is_empty())
        {
            Some(encoded) => match serde_json::from_str::<Value>(&encoded) {
                Ok(value) => Some(value),
                Err(error) => {
                    return Err(vec![format!(
                        "WC_SERVICE_API_DEPLOYERS must be a JSON object: {error}"
                    )])
                }
            },
            None => crate::config_file::get("service_api.deployers"),
        };
        parse_service_deployers(configured.as_ref())
    });

pub fn service_api_deployers(
) -> Result<&'static BTreeMap<String, ServiceDeployer>, &'static [String]> {
    match &*SERVICE_API_DEPLOYERS {
        Ok(deployers) => Ok(deployers),
        Err(problems) => Err(problems.as_slice()),
    }
}

pub fn service_deployer_for(service: &str, action: &str) -> Option<&'static ServiceDeployer> {
    service_api_deployers()
        .ok()?
        .values()
        .find(|deployer| deployer.allows(service, action))
}
