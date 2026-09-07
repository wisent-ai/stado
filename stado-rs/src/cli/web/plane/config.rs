//! Reading and writing the `web_api` section of the deployment configuration.

use serde_json::{json, Map, Value};

use crate::cli::CmdError;
use crate::config::WebApiProduct;

/// One declared product, read through the configuration plane so the parser
/// that validates a declaration is the only thing that interprets one.
pub(crate) fn product(name: &str) -> Result<&'static WebApiProduct, CmdError> {
    let products = crate::config::web_api_products()
        .map_err(|problems| CmdError::click(problems.join("; ")))?;
    products.get(name).ok_or_else(|| {
        CmdError::usage(format!(
            "no web product {name:?} is declared; declared: {}",
            if products.is_empty() {
                "none".to_string()
            } else {
                products.keys().cloned().collect::<Vec<_>>().join(", ")
            }
        ))
    })
}

/// Load the config file, apply one mutation under `web_api`, refuse anything
/// the plane's own parser rejects, and write atomically.
///
/// The same shape as `stado database`'s mutation, deliberately: a second way
/// to write the configuration is a second thing that can write it wrongly.
pub(crate) fn mutate_web<F>(section: &str, mutation: F) -> Result<Value, CmdError>
where
    F: FnOnce(&mut Map<String, Value>) -> Result<(), String>,
{
    let path = crate::config_file::config_path()
        .map_err(|error| CmdError::click(error.to_string()))?
        .ok_or_else(|| CmdError::click("no config file exists; run: stado config init"))?;
    let original = std::fs::read_to_string(&path)?;
    let mut document: Value =
        serde_json::from_str(&original).map_err(|error| CmdError::click(error.to_string()))?;
    if !document.is_object() {
        return Err(CmdError::click("config file must contain a JSON object"));
    }
    let web_api = document
        .as_object_mut()
        .expect("checked above")
        .entry("web_api".to_string())
        .or_insert_with(|| json!({}));
    if !web_api.is_object() {
        return Err(CmdError::click("web_api must be an object"));
    }
    let entry = web_api
        .as_object_mut()
        .expect("checked above")
        .entry(section.to_string())
        .or_insert_with(|| json!({}));
    let map = entry
        .as_object_mut()
        .ok_or_else(|| CmdError::click(format!("web_api.{section} must be an object")))?;
    mutation(map)?;
    // The parsers refuse an empty map, so a removal that empties the plane
    // collapses the section rather than leaving a document nothing validates.
    if map.is_empty() {
        web_api
            .as_object_mut()
            .expect("checked above")
            .remove(section);
    }
    if web_api
        .as_object()
        .is_some_and(|section| section.is_empty())
    {
        document
            .as_object_mut()
            .expect("checked above")
            .remove("web_api");
    }

    let problems = crate::config_file::validate(&document);
    if !problems.is_empty() {
        return Err(CmdError::click(format!(
            "rejected, config unchanged: {}",
            problems.join("; ")
        )));
    }
    let body = format!("{}\n", serde_json::to_string_pretty(&document)?);
    let temporary = std::path::PathBuf::from(format!("{}.web-setting", path.display()));
    std::fs::write(&temporary, body)?;
    if let Ok(metadata) = std::fs::metadata(&path) {
        std::fs::set_permissions(&temporary, metadata.permissions())?;
    }
    std::fs::rename(&temporary, &path)?;
    Ok(document)
}
