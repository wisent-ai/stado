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
    // Which deployer each service belongs to. A service is a conflict only
    // when two different deployers name it; one deployer that still lists
    // several labels its product ran under before names its one catalog unit
    // once, however many of those labels it carries.
    let mut services_seen: BTreeMap<String, String> = BTreeMap::new();
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
        if entry.contains_key("consumer") {
            problems.push(format!(
                "service_api.deployers.{product}.consumer is retired; configure \
                 Stado's secrets.skarbiec identity instead"
            ));
            entry_valid = false;
        }
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        #[allow(dead_code)]
        struct Fields {
            item: Option<serde::de::IgnoredAny>,
            services: Option<serde::de::IgnoredAny>,
            actions: Option<serde::de::IgnoredAny>,
            /// Retired; refused above with its replacement named.
            consumer: Option<serde::de::IgnoredAny>,
        }
        entry_valid &= crate::config::boundaries::declared_fields::<Fields>(
            raw_entry,
            &format!("service_api.deployers.{product}"),
            &mut problems,
        );
        // The item is the product's own name, compared as the string it is;
        // an absent item or one of another type is named as such.
        match entry.get("item") {
            Some(Value::String(item)) if item == product => {}
            Some(Value::String(item)) => {
                problems.push(format!(
                    "service_api.deployers.{product}.item must be the product's own name {product:?}, not {item:?}"
                ));
                entry_valid = false;
            }
            Some(other) => {
                problems.push(format!(
                    "service_api.deployers.{product}.item must be the string {product:?}, not {other}"
                ));
                entry_valid = false;
            }
            None => {
                problems.push(format!(
                    "service_api.deployers.{product}.item is required and must be {product:?}"
                ));
                entry_valid = false;
            }
        }
        let item = product.as_str();
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
                    let Some(declared) = value.as_str() else {
                        problems.push(format!(
                            "service_api.deployers.{product}.services entries must be strings"
                        ));
                        entry_valid = false;
                        continue;
                    };
                    // A product runs as one unit per host, so a label under a
                    // product's deployer that is no product's unit is a label
                    // that product ran under before: it is read as the
                    // product's one catalog unit.
                    let current =
                        match crate::deploy::service_catalog::superseded_label(product, declared) {
                            Ok(entry) => entry.and_then(|entry| entry.unit),
                            Err(error) => {
                                problems.push(format!(
                                "service_api.deployers.{product}.services {declared:?} could not \
                                 be checked against the product catalog: {error}"
                            ));
                                entry_valid = false;
                                None
                            }
                        };
                    // A superseded label reads as the product's current unit;
                    // a label the catalog does not supersede is itself.
                    let service = match current.as_deref() {
                        Some(unit) => unit,
                        None => declared,
                    };
                    let unit_label = service.strip_prefix("com.wisent.").is_some_and(canonical);
                    if !canonical(service) && !unit_label {
                        problems.push(format!(
                            "service_api.deployers.{product}.services contains non-canonical {service:?}"
                        ));
                        entry_valid = false;
                    }
                    match services_seen.get(service) {
                        Some(owner) if owner != product => {
                            problems.push(format!(
                                "service {service:?} is mapped to more than one deployer: \
                                 {owner} and {product}"
                            ));
                            entry_valid = false;
                        }
                        Some(_) => continue,
                        None => {
                            services_seen.insert(service.to_string(), product.to_string());
                        }
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
    if problems.is_empty() {
        Ok(deployers)
    } else {
        Err(problems)
    }
}

static SERVICE_API_DEPLOYERS: LazyLock<Result<BTreeMap<String, ServiceDeployer>, Vec<String>>> =
    LazyLock::new(|| {
        // `WC_SERVICE_API_DEPLOYERS` overrides the config file's block when
        // it is set and not blank; unset or blank, the file is read. A value
        // that is not Unicode cannot be JSON and is refused as such.
        let configured = match std::env::var("WC_SERVICE_API_DEPLOYERS") {
            Ok(encoded) if !encoded.trim().is_empty() => {
                match serde_json::from_str::<Value>(&encoded) {
                    Ok(value) => Some(value),
                    Err(error) => {
                        return Err(vec![format!(
                            "WC_SERVICE_API_DEPLOYERS must be a JSON object: {error}"
                        )])
                    }
                }
            }
            Ok(_) | Err(std::env::VarError::NotPresent) => {
                crate::config_file::get("service_api.deployers")
            }
            Err(std::env::VarError::NotUnicode(raw)) => {
                return Err(vec![format!(
                    "WC_SERVICE_API_DEPLOYERS must be a JSON object, and is not Unicode: {raw:?}"
                )])
            }
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
