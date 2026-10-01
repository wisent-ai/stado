//! The build-time variables the platform declares.

use std::path::Path;

use serde_json::Value;

use crate::cli::CmdError;

/// The build-time variables the platform declares in `.wisent-release.json`.
///
/// Read from the manifest rather than from the worker's environment because
/// the release worker passes `secret_env` and nothing else: a Skarbiec value
/// has to travel out of band, while a public constant is already in the file
/// this command reads for the product name. One document, two fields, one
/// reader.
pub(in crate::cli::web::builds) fn declared_env(
    source: &Path,
    platform: &str,
) -> Result<Vec<(String, String)>, CmdError> {
    let path = source.join(crate::release_pipeline::PRODUCT_MANIFEST);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(Vec::new());
    };
    let declared: Value = serde_json::from_str(&text).map_err(|error| {
        CmdError::click(format!("{} is not valid JSON: {error}", path.display()))
    })?;
    let Some(Value::Object(entries)) = declared
        .get("platforms")
        .and_then(|platforms| platforms.get(platform))
        .and_then(|recipe| recipe.get("env"))
        .cloned()
    else {
        return Ok(Vec::new());
    };
    let mut pairs = Vec::new();
    for (name, value) in entries {
        let value = value.as_str().ok_or_else(|| {
            CmdError::click(format!(
                "{}: platform {platform} declares env.{name} as something other than a string",
                path.display()
            ))
        })?;
        pairs.push((name, value.to_string()));
    }
    Ok(pairs)
}
