//! Product delivery destinations belong to the canonical registry, not source.

use super::validate::predicates::identifier;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeSet;

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
            Err(format!(
                "invalid release destination {:?} on platform {:?}",
                self.target, self.platform
            ))
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
    document
        .get(FIELD)
        .map(|value| {
            value
                .as_object()
                .ok_or_else(|| format!("registry.{FIELD} must be an object"))
        })
        .transpose()
}

/// The hosts one product's release is delivered to.
///
/// A product with a `release_delivery_targets` entry goes to exactly that
/// list: the declaration that places a product on a host that does not run
/// it yet. A product with none goes to every host that runs it, read from the
/// registry's own record of that, `targets[].managed_versions.<product>`, so
/// a published release installs itself where the product already runs and
/// nobody lists those hosts by hand; a host that adopts the product joins the
/// next release's placement by that same record.
fn inspect(
    document: &Value,
    product: &str,
    mut accept: impl FnMut(&str, &str),
) -> Result<(), String> {
    validate_product(product)?;
    let targets = document["targets"]
        .as_array()
        .ok_or("registry.targets must be an array")?;
    let declared = declarations(document)?.and_then(|products| products.get(product));
    let names: Vec<&str> =
        match declared {
            Some(names) => {
                let names = names
                    .as_array()
                    .filter(|names| !names.is_empty())
                    .ok_or_else(|| {
                        format!("registry.{FIELD}.{product} must contain a nonempty target list")
                    })?;
                let mut listed = Vec::with_capacity(names.len());
                for name in names {
                    listed.push(name.as_str().filter(|name| identifier(name)).ok_or_else(
                        || format!("registry.{FIELD}.{product} contains an invalid target"),
                    )?);
                }
                listed
            }
            None => running(targets, product),
        };
    if names.is_empty() {
        return Err(format!(
            "no host runs {product} (no registry target records managed_versions.{product}) and \
             registry.{FIELD} declares no target for it, so its release has nowhere to go; \
             install it on a host, or place it with stado release destinations set {product} \
             --target <HOST>"
        ));
    }
    let mut seen = BTreeSet::new();
    for name in names {
        if !seen.insert(name) {
            return Err(format!("registry.{FIELD}.{product} repeats target {name}"));
        }
        let target = targets
            .iter()
            .find(|target| target["name"] == name)
            .ok_or_else(|| format!("registry.{FIELD}.{product} names missing target {name}"))?;
        let platform = target["release_platform"]
            .as_str()
            .filter(|platform| !platform.is_empty())
            .ok_or_else(|| {
                format!("release destination {name} has no declared release_platform")
            })?;
        accept(name, platform);
    }
    Ok(())
}

/// The registry targets that run `product`: each whose `managed_versions`
/// names it.
fn running<'a>(targets: &'a [Value], product: &str) -> Vec<&'a str> {
    targets
        .iter()
        .filter(|target| {
            target["managed_versions"]
                .get(product)
                .and_then(Value::as_str)
                .is_some_and(|version| !version.is_empty())
        })
        .filter_map(|target| target["name"].as_str())
        .collect()
}

pub fn read(document: &Value, product: &str) -> Result<Vec<Destination>, String> {
    let mut destinations = Vec::new();
    inspect(document, product, |target, platform| {
        destinations.push(Destination {
            target: target.to_owned(),
            platform: platform.to_owned(),
        })
    })?;
    destinations.sort_unstable_by(|left, right| left.target.cmp(&right.target));
    Ok(destinations)
}

/// Whether `product`'s destinations are a declared list rather than the hosts
/// that run it.
pub fn declared(document: &Value, product: &str) -> Result<bool, String> {
    Ok(declarations(document)?.is_some_and(|products| products.contains_key(product)))
}

pub fn validate(document: &Value) -> Result<(), String> {
    if let Some(products) = declarations(document)? {
        for product in products.keys() {
            inspect(document, product, |_, _| {})?;
        }
    }
    Ok(())
}
