//! Service boundary: deployers, their actions and its Skarbiec grant.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use crate::config::skarbiec_url;
use crate::config_file::{expand_tilde, resolve as cfg};
use serde_json::Value;

pub const SERVICE_API_VERIFIER_CONSUMER: &str = "stado-service-api-verifier";
pub const SERVICE_API_ACTIONS: &[&str] = &["status", "restart", "promote", "reconcile"];
pub const ACTIVE_DEPLOYED_SERVICES: &[&str] = &["com.wisent.weles-api", "image-video-router"];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceDeployer {
    item: String,
    consumer: String,
    services: Vec<String>,
    actions: Vec<String>,
}

impl ServiceDeployer {
    pub fn item(&self) -> &str {
        &self.item
    }

    pub fn consumer(&self) -> &str {
        &self.consumer
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
                "service_api.deployers.{product} must contain consumer, item, services, and actions"
            ));
            continue;
        };
        for key in entry.keys() {
            if !matches!(key.as_str(), "consumer" | "item" | "services" | "actions") {
                problems.push(format!(
                    "service_api.deployers.{product} contains unsupported key {key:?}"
                ));
                entry_valid = false;
            }
        }
        let item = entry
            .get("item")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let consumer = entry
            .get("consumer")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !canonical(item) || !item.ends_with("-deployer") {
            problems.push(format!(
                "service_api.deployers.{product}.item must name one canonical *-deployer item"
            ));
            entry_valid = false;
        }
        if consumer != item {
            problems.push(format!(
                "service_api.deployers.{product}.consumer must equal its exact item {item:?}"
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
                    if !SERVICE_API_ACTIONS.contains(&action) || !seen.insert(action.to_string()) {
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
                    consumer: consumer.to_string(),
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
static SERVICE_SKARBIEC_URL: LazyLock<String> = LazyLock::new(|| {
    cfg(
        "WC_SERVICE_SKARBIEC_URL",
        "service_api.skarbiec.url",
        skarbiec_url(),
    )
});
static SERVICE_SKARBIEC_CONSUMER: LazyLock<String> = LazyLock::new(|| {
    cfg(
        "WC_SERVICE_SKARBIEC_CONSUMER",
        "service_api.skarbiec.consumer",
        SERVICE_API_VERIFIER_CONSUMER,
    )
});
static SERVICE_SKARBIEC_TOKEN_FILE: LazyLock<String> = LazyLock::new(|| {
    let default = std::env::var("HOME")
        .map(|home| {
            std::path::Path::new(&home)
                .join(".stado")
                .join("stado-service-api-verifier-skarbiec-token")
                .to_string_lossy()
                .into_owned()
        })
        .unwrap_or_default();
    expand_tilde(&cfg(
        "WC_SERVICE_SKARBIEC_TOKEN_FILE",
        "service_api.skarbiec.token_file",
        &default,
    ))
    .to_string_lossy()
    .into_owned()
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

pub fn service_skarbiec_url() -> &'static str {
    SERVICE_SKARBIEC_URL.as_str()
}

pub fn service_skarbiec_consumer() -> &'static str {
    SERVICE_SKARBIEC_CONSUMER.as_str()
}

pub fn service_skarbiec_token_file() -> &'static str {
    SERVICE_SKARBIEC_TOKEN_FILE.as_str()
}
