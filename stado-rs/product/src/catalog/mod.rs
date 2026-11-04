mod command;
pub mod committed;
mod names;
mod rivals;
mod types;
mod validation;
use anyhow::{bail, Context, Result};
pub use command::{cli_catalog, docs_origin, organization_repositories, run, services};
/// Every rival claim of a product with its source fetched again.
pub use rivals::check_claims;
use serde_json::{json, Value};
use std::{fs, path::Path};
pub use types::{
    list_field, DeclaredKeychain, Dependency, Family, RecipeKind, Visibility, FEATURES,
};
pub use validation::submit::{policy as submit_policy, Criticality, SubmitPolicy};
pub use validation::{validate, InvalidCatalog};

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

/// The catalog a reader acts on; [`read`] without saying where it came from.
pub fn current(runtime: &crate::common::Runtime) -> Result<Value> {
    read(runtime).map(|(document, _)| document)
}

/// The catalog a reader acts on, and where it was read. A catalog file a
/// checkout tracks is read at that checkout's committed head (or its
/// `origin/main` when the head is behind), as an installation reads it, so
/// an uncommitted edit one session is half way through never refuses or
/// steers another session's read; the working tree's edit is named on
/// stderr instead ([`committed::report_working_tree`]). A file no checkout
/// tracks — a catalog an operator or a test names with `--catalog` — is read
/// as it stands, and no catalog file at all is the embedded one.
pub fn read(runtime: &crate::common::Runtime) -> Result<(Value, Option<committed::Provenance>)> {
    if runtime.embedded_catalog {
        return Ok((embedded()?, None));
    }
    if committed::tracked(&runtime.catalog)?.is_none() {
        return Ok((load(&runtime.catalog)?, None));
    }
    let (document, read) = committed::for_installation(runtime, "", None)?;
    committed::report_working_tree(&runtime.catalog, &read, &document);
    Ok((document, Some(read)))
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

/// Every product as `catalog --json` serves it: the whole validated record,
/// and every roadmap status with whether it is decided
/// (`roadmap_statuses`), so a reader tells open roadmap items from ended
/// ones by the catalog's own declaration. A projection here dropped every
/// field it did not list, so the rivals, the benchmark and the roadmap a
/// record declares never reached Probierz, which reads them from this answer.
pub fn rows(value: &Value) -> Result<Value> {
    let products = value["products"]
        .as_array()
        .context("products must be an array")?;
    Ok(json!({ "products": products, "roadmap_statuses": types::RoadmapStatus::published() }))
}
