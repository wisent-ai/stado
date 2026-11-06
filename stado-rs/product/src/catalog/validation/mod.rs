use super::text;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;

mod naming;
mod public_status;
mod record;
mod roadmap;
mod service;
pub mod submit;

use record::record;

/// The field parses as `T`, or the refusal names the field and the word.
fn choice<'a, T: Deserialize<'a>>(value: &'a Value, field: &str) -> Result<T> {
    T::deserialize(&value[field])
        .with_context(|| format!("{field}: {} is not one of its values", value[field]))
}

/// A catalog whose declarations do not hold: a product lacking a field it
/// must declare, a value outside its choices, two products of one id. It is
/// the declared state refusing, so a caller classes it `config` and no retry
/// changes it; the cause chain says which product and field.
#[derive(Debug)]
pub struct InvalidCatalog;

impl std::fmt::Display for InvalidCatalog {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("the product catalog does not hold its declarations")
    }
}

impl std::error::Error for InvalidCatalog {}

/// [`declarations`], with a failure typed [`InvalidCatalog`].
pub fn validate(document: &Value) -> Result<()> {
    declarations(document).map_err(|cause| cause.context(InvalidCatalog))
}

fn declarations(document: &Value) -> Result<()> {
    let products = document["products"]
        .as_array()
        .context("products must be a list")?;
    let mut ids = HashSet::new();
    let mut targets = Vec::new();
    let mut dependencies = Vec::new();
    for (place, product) in products.iter().enumerate() {
        let id = text(product, "id").with_context(|| format!("products[{place}]"))?;
        record(product, id, &mut ids, &mut targets, &mut dependencies)
            .with_context(|| format!("product {id}"))?;
    }
    for (id, target) in targets {
        if !ids.contains(target) {
            bail!("{id}: integration names unknown product {target}");
        }
    }
    for (id, target) in dependencies {
        if !ids.contains(target.as_str()) {
            bail!("{id}: dependency names unknown product {target}");
        }
    }
    naming::validate(products)?;
    Ok(())
}

fn strings(value: &Value, field: &str) -> Result<()> {
    let values = value[field]
        .as_array()
        .with_context(|| format!("{field} must be a list"))?;
    if values.is_empty()
        || values
            .iter()
            .any(|v| v.as_str().is_none_or(|s| s.trim().is_empty()))
    {
        bail!("{field} requires non-empty strings");
    }
    Ok(())
}

/// `OWNER/REPO`: exactly two names, each of letters, digits, `-`, `_` and `.`,
/// and neither `.` nor `..`.
fn repository(value: &str) -> Result<()> {
    let named = |part: &str| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && part
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
    };
    match value.split_once('/') {
        Some((owner, repo)) if named(owner) && named(repo) => Ok(()),
        _ => bail!("invalid repository {value:?}; expected OWNER/REPO"),
    }
}
