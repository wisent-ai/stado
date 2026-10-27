//! Reading and writing the `web_api` section of the deployment configuration.

use serde_json::{json, Map, Value};

use crate::cli::CmdError;
use crate::config::WebApiProduct;

/// One declared product, read through the configuration plane so the parser
/// that validates a declaration is the only thing that interprets one.
pub(crate) fn product(name: &str) -> Result<&'static WebApiProduct, CmdError> {
    let products = crate::config::web_api_products()
        .map_err(|problems| CmdError::declaration(problems.join("; ")))?;
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
        .map_err(CmdError::from)?
        .ok_or_else(|| {
            CmdError::click("no config file exists; run: stado config init")
                .stating(crate::primitives::failure::FailureCode::Config)
        })?;
    let original = std::fs::read_to_string(&path)?;
    let Value::Object(mut document) =
        serde_json::from_str::<Value>(&original).map_err(CmdError::from)?
    else {
        return Err(CmdError::click("config file must contain a JSON object")
            .stating(crate::primitives::failure::FailureCode::Config));
    };
    let Value::Object(web_api) = document
        .entry("web_api".to_string())
        .or_insert_with(|| json!({}))
    else {
        return Err(CmdError::click("web_api must be an object")
            .stating(crate::primitives::failure::FailureCode::Config));
    };
    let Value::Object(map) = web_api
        .entry(section.to_string())
        .or_insert_with(|| json!({}))
    else {
        return Err(CmdError::click(format!("web_api.{section} must be an object"))
            .stating(crate::primitives::failure::FailureCode::Config));
    };
    mutation(map)?;
    // The parsers refuse an empty map, so a removal that empties the plane
    // collapses the section rather than leaving a document nothing validates.
    if map.is_empty() {
        web_api.remove(section);
    }
    if web_api.is_empty() {
        document.remove("web_api");
    }
    let document = Value::Object(document);

    let problems = crate::config_file::validate(&document);
    if !problems.is_empty() {
        return Err(CmdError::refused(format!(
            "rejected, config unchanged: {}",
            problems.join("; ")
        )));
    }
    crate::config_file::replace(&path, &document)?;
    Ok(document)
}
