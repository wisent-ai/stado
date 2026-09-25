mod command;
mod types;
mod validation;
use anyhow::{bail, Context, Result};
pub use command::{cli_catalog, run, services};
use serde_json::{json, Value};
use std::{fs, path::Path};
pub use validation::validate;

/// The authority at the root of the Stado repository, compiled into the binary.
pub const EMBEDDED: &str = include_str!("../../../../catalog/products.yml");

pub fn load(path: &Path) -> Result<Value> {
    let bytes = fs::read(path)
        .with_context(|| format!("reading authoritative catalog {}", path.display()))?;
    let value: Value =
        serde_yaml::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))?;
    validate(&value)?;
    Ok(value)
}

pub fn current(runtime: &crate::common::Runtime) -> Result<Value> {
    if !runtime.embedded_catalog {
        return load(&runtime.catalog);
    }
    embedded()
}

/// The catalog compiled into this build, validated.
pub fn embedded() -> Result<Value> {
    let document: Value = serde_yaml::from_str(EMBEDDED)?;
    validate(&document)?;
    Ok(document)
}

pub fn product<'a>(catalog: &'a Value, id: &str) -> Result<&'a Value> {
    catalog["products"]
        .as_array()
        .context("products must be an array")?
        .iter()
        .find(|product| product["id"] == id)
        .with_context(|| format!("no catalogued product {id}"))
}

pub fn text<'a>(value: &'a Value, field: &str) -> Result<&'a str> {
    let text = value[field]
        .as_str()
        .with_context(|| format!("{field}: expected a string"))?;
    if text.trim().is_empty() {
        bail!("{field}: expected a non-empty string");
    }
    Ok(text)
}

pub fn rows(value: &Value) -> Result<Value> {
    Ok(
        json!({"products": value["products"].as_array().context("products must be an array")?.iter()
        .map(|product| json!({"id": product["id"], "name": product["name"], "family": product["family"],
            "description": product["description"], "surfaces": product["surfaces"], "installations": product["installations"]}))
        .collect::<Vec<_>>() }),
    )
}
