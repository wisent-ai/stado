mod command;
pub mod committed;
mod names;
mod rivals;
mod types;
mod validation;
use anyhow::{bail, Context, Result};
pub use command::{cli_catalog, run, services};
use serde_json::{json, Value};
use std::{fs, path::Path};
pub use types::{
    list_field, DeclaredKeychain, Dependency, Family, RecipeKind, Visibility, FEATURES,
};
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

/// The `schema_version` the compiled catalog is written in: the one schema
/// this build reads, read once from the catalog itself rather than restated.
fn embedded_schema() -> Result<&'static Value> {
    static SCHEMA: std::sync::LazyLock<Result<Value, String>> = std::sync::LazyLock::new(|| {
        serde_yaml::from_str::<Value>(EMBEDDED)
            .map(|document| document["schema_version"].clone())
            .map_err(|error| {
                format!("the catalog compiled into this build does not parse: {error}")
            })
    });
    SCHEMA.as_ref().map_err(|error| anyhow::anyhow!("{error}"))
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

/// Every product as `catalog --json` serves it: the whole validated record.
/// A projection here dropped every field it did not list, so the rivals, the
/// benchmark and the roadmap a record declares never reached Probierz, which
/// reads them from this answer.
pub fn rows(value: &Value) -> Result<Value> {
    let products = value["products"]
        .as_array()
        .context("products must be an array")?;
    Ok(json!({ "products": products }))
}
