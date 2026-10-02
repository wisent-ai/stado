//! Product delivery destinations belong to the canonical registry, not source.

use std::collections::BTreeSet;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use super::validate::predicates::identifier;

pub const FIELD: &str = "release_delivery_targets";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Destination {
    pub target: String,
    pub platform: String,
}

impl Destination {
    pub fn validate(&self) -> Result<(), String> {
        if identifier(&self.target) && identifier(&self.platform) {
            Ok(())
        } else {
            Err(format!("invalid release destination {:?} on platform {:?}", self.target, self.platform))
        }
    }
}

pub fn validate_product(product: &str) -> Result<(), String> {
    if identifier(product) {
        Ok(())
    } else {
        Err("release destination product is not a canonical identifier".into())
    }
}

pub fn declarations(document: &Value) -> Result<Option<&Map<String, Value>>, String> {
    document.get(FIELD).map(|value| value.as_object()
        .ok_or_else(|| format!("registry.{FIELD} must be an object"))).transpose()
}

fn inspect(document: &Value, product: &str, mut accept: impl FnMut(&str, &str)) -> Result<(), String> {
    validate_product(product)?;
    let names = declarations(document)?.and_then(|products| products.get(product))
        .ok_or_else(|| format!(
            "no release delivery targets declared for {product}; use stado release destinations set {product} --target <HOST>"
        ))?.as_array().filter(|names| !names.is_empty())
        .ok_or_else(|| format!("registry.{FIELD}.{product} must contain a nonempty target list"))?;
    let targets = document["targets"].as_array().ok_or("registry.targets must be an array")?;
    let mut seen = BTreeSet::new();
    for name in names {
        let name = name.as_str().filter(|name| identifier(name))
            .ok_or_else(|| format!("registry.{FIELD}.{product} contains an invalid target"))?;
        if !seen.insert(name) {
            return Err(format!("registry.{FIELD}.{product} repeats target {name}"));
        }
        let target = targets.iter().find(|target| target["name"] == name)
            .ok_or_else(|| format!("registry.{FIELD}.{product} names missing target {name}"))?;
        let platform = target["release_platform"].as_str().filter(|platform| !platform.is_empty())
            .ok_or_else(|| format!("release destination {name} has no declared release_platform"))?;
        accept(name, platform);
    }
    Ok(())
}

pub fn read(document: &Value, product: &str) -> Result<Vec<Destination>, String> {
    let mut destinations = Vec::new();
    inspect(document, product, |target, platform| destinations.push(Destination {
        target: target.to_owned(), platform: platform.to_owned(),
    }))?;
    destinations.sort_unstable_by(|left, right| left.target.cmp(&right.target));
    Ok(destinations)
}

pub fn validate(document: &Value) -> Result<(), String> {
    if let Some(products) = declarations(document)? {
        for product in products.keys() {
            inspect(document, product, |_, _| {})?;
        }
    }
    Ok(())
}
