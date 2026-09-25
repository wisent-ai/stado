//! Authenticated release publishers and their declared products. A product
//! publishes as itself: its bearer is the vault item named after the product,
//! read through Stado's Skarbiec identity.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use super::ACTIVE_RELEASE_PUBLISHERS;
use serde_json::Value;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleasePublisher {
    item: String,
    prefix: String,
}

impl ReleasePublisher {
    pub fn item(&self) -> &str {
        &self.item
    }

    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    pub fn allows_key(&self, key: &str) -> bool {
        key.starts_with(&self.prefix)
    }

    /// The authorized release listing prefix, with the caller's trailing `/`
    /// intact for the reason [`ObjectApiNamespace::authorized_list_prefix`]
    /// gives: the separator decides whether a scan stays inside a coordinate
    /// or reaches every sibling whose name begins with it.
    pub fn authorized_list_prefix(&self, requested: &str) -> Option<String> {
        let requested = requested.trim_start_matches('/');
        let path = requested.trim_end_matches('/');
        let root = self.prefix.strip_suffix('/').unwrap_or(&self.prefix);
        if path == root {
            Some(self.prefix.clone())
        } else if path.starts_with(&self.prefix) {
            Some(requested.to_string())
        } else {
            None
        }
    }
}

pub(crate) fn parse_release_publishers(
    value: Option<&Value>,
) -> Result<BTreeMap<String, ReleasePublisher>, Vec<String>> {
    let publishers = parse_declared_release_publishers(value)?;
    let problems = missing_release_publishers(&publishers);
    if problems.is_empty() {
        Ok(publishers)
    } else {
        Err(problems)
    }
}

fn parse_declared_release_publishers(
    value: Option<&Value>,
) -> Result<BTreeMap<String, ReleasePublisher>, Vec<String>> {
    let Some(Value::Object(entries)) = value else {
        return Err(vec![
            "release_api.publishers must be a non-empty product-to-item mapping".to_string(),
        ]);
    };
    if entries.is_empty() {
        return Err(vec![
            "release_api.publishers must not be empty; authenticated release writes fail closed"
                .to_string(),
        ]);
    }

    let mut problems = Vec::new();
    let mut publishers = BTreeMap::new();
    let mut items = BTreeSet::new();
    let mut prefixes = BTreeSet::new();
    for (product, raw_entry) in entries {
        let mut entry_valid = true;
        if product.trim() != product
            || crate::remote::object_store::ObjectRef::new(product, "sentinel").is_err()
        {
            problems.push(format!(
                "release_api.publishers key {product:?} is not a canonical product name"
            ));
            entry_valid = false;
        }
        let Some(entry) = raw_entry.as_object() else {
            problems.push(format!(
                "release_api.publishers.{product} must be an object with item and prefix"
            ));
            continue;
        };
        for key in entry.keys() {
            if key != "item" && key != "prefix" {
                problems.push(format!(
                    "release_api.publishers.{product} contains unsupported key {key:?}"
                ));
                entry_valid = false;
            }
        }
        let item = entry
            .get("item")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let expected_item = product.as_str();
        if item != expected_item {
            problems.push(format!(
                "release_api.publishers.{product}.item must be the product's own name \
                 {expected_item:?}, got {item:?}"
            ));
            entry_valid = false;
        }
        let prefix = entry
            .get("prefix")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let expected_prefix = format!("{product}/");
        if prefix != expected_prefix {
            problems.push(format!(
                "release_api.publishers.{product}.prefix must be {expected_prefix:?}, got {prefix:?}"
            ));
            entry_valid = false;
        }
        if !items.insert(item.to_string()) {
            problems.push(format!(
                "release_api.publishers maps more than one product to item {item:?}"
            ));
            entry_valid = false;
        }
        if !prefixes.insert(prefix.to_string()) {
            problems.push(format!(
                "release_api.publishers maps more than one product to prefix {prefix:?}"
            ));
            entry_valid = false;
        }
        if entry_valid {
            publishers.insert(
                product.to_string(),
                ReleasePublisher {
                    item: item.to_string(),
                    prefix: prefix.to_string(),
                },
            );
        }
    }
    if problems.is_empty() {
        Ok(publishers)
    } else {
        Err(problems)
    }
}

fn missing_release_publishers(publishers: &BTreeMap<String, ReleasePublisher>) -> Vec<String> {
    ACTIVE_RELEASE_PUBLISHERS
        .iter()
        .filter(|&&required| !publishers.contains_key(required))
        .map(|required| format!("release_api.publishers is missing active publisher {required:?}"))
        .collect()
}

static RELEASE_API_PUBLISHERS: LazyLock<Result<BTreeMap<String, ReleasePublisher>, Vec<String>>> =
    LazyLock::new(|| {
        let configured = match std::env::var("WC_RELEASE_API_PUBLISHERS")
            .ok()
            .filter(|value| !value.trim().is_empty())
        {
            Some(encoded) => match serde_json::from_str::<Value>(&encoded) {
                Ok(value) => Some(value),
                Err(error) => {
                    return Err(vec![format!(
                        "WC_RELEASE_API_PUBLISHERS must be a JSON object: {error}"
                    )])
                }
            },
            None => crate::config_file::get("release_api.publishers"),
        };
        parse_declared_release_publishers(configured.as_ref())
    });
static RELEASE_API_PUBLISHER_REQUIREMENTS: LazyLock<Vec<String>> =
    LazyLock::new(|| match &*RELEASE_API_PUBLISHERS {
        Ok(publishers) => missing_release_publishers(publishers),
        Err(_) => Vec::new(),
    });

pub fn release_api_publishers(
) -> Result<&'static BTreeMap<String, ReleasePublisher>, &'static [String]> {
    match &*RELEASE_API_PUBLISHERS {
        Ok(publishers) if RELEASE_API_PUBLISHER_REQUIREMENTS.is_empty() => Ok(publishers),
        Ok(_) => Err(RELEASE_API_PUBLISHER_REQUIREMENTS.as_slice()),
        Err(problems) => Err(problems.as_slice()),
    }
}

/// A publishing client needs only its declared products; the serving API still
/// requires its complete active publisher table through `release_api_publishers`.
///
/// A declaration carries nothing the product name does not: its item must be
/// the product and its prefix `<product>/`. So a key whose product this host
/// has not declared yet still resolves to that product's publisher, and the
/// caller reads the product's own bearer instead of refusing. Whether the
/// bearer exists and the serving API accepts it is the serving side's answer;
/// `stado build submit` declares the publisher before its first write when
/// this host's table lacks it (`release_catalog::ensure_publisher`).
pub fn release_client_publisher_for_key(
    key: &str,
) -> Result<Option<ReleasePublisher>, &'static [String]> {
    match &*RELEASE_API_PUBLISHERS {
        Ok(publishers) => Ok(publishers
            .values()
            .find(|publisher| publisher.allows_key(key))
            .cloned()
            .or_else(|| derived_publisher(key))),
        Err(problems) => Err(problems.as_slice()),
    }
}

/// The publisher a product's declaration would name, from the first segment
/// of `key`: item `<product>`, prefix `<product>/`.
fn derived_publisher(key: &str) -> Option<ReleasePublisher> {
    let (product, rest) = key.trim_start_matches('/').split_once('/')?;
    if product.is_empty()
        || rest.is_empty()
        || crate::remote::object_store::ObjectRef::new(product, "sentinel").is_err()
    {
        return None;
    }
    Some(ReleasePublisher {
        item: product.to_string(),
        prefix: format!("{product}/"),
    })
}

/// Whether this host's configuration declares `product`'s release publisher.
pub fn release_publisher_declared(product: &str) -> bool {
    matches!(&*RELEASE_API_PUBLISHERS, Ok(publishers) if publishers.contains_key(product))
}

/// The serving API's publisher for `key`. A product declared after this
/// process loaded its configuration is looked up again in the file as it is
/// on disk now, so `ensure_publisher`'s declaration takes effect on the
/// serving hosts without restarting them; the declared table is still the
/// only authority, and a key no declaration names is still refused.
pub fn release_publisher_for_key(key: &str) -> Option<ReleasePublisher> {
    let find = |publishers: &BTreeMap<String, ReleasePublisher>| {
        publishers
            .values()
            .find(|publisher| publisher.allows_key(key))
            .cloned()
    };
    find(release_api_publishers().ok()?).or_else(|| find(&fresh_release_publishers()?))
}

pub fn release_publisher_for_list(prefix: &str) -> Option<(ReleasePublisher, String)> {
    let find = |publishers: &BTreeMap<String, ReleasePublisher>| {
        publishers.values().find_map(|publisher| {
            publisher
                .authorized_list_prefix(prefix)
                .map(|authorized| (publisher.clone(), authorized))
        })
    };
    find(release_api_publishers().ok()?).or_else(|| find(&fresh_release_publishers()?))
}

/// The publisher table in the config file as it is now, when the table is
/// read from the file at all (`WC_RELEASE_API_PUBLISHERS` pins it for the
/// process's lifetime) and is complete.
fn fresh_release_publishers() -> Option<BTreeMap<String, ReleasePublisher>> {
    let pinned = std::env::var("WC_RELEASE_API_PUBLISHERS")
        .ok()
        .is_some_and(|value| !value.trim().is_empty());
    if pinned {
        return None;
    }
    parse_release_publishers(crate::config_file::get_fresh("release_api.publishers").as_ref()).ok()
}
