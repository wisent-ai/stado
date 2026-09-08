//! The product's `package.json`: reading it, and the scripts it declares.

pub(in crate::cli::web::builds) mod kind;
pub(in crate::cli::web::builds) mod version;

use std::path::Path;

use serde_json::{Map, Value};

use crate::cli::CmdError;

/// `package.json` as an object, with every way it can be unusable named.
fn manifest(source: &Path) -> Result<Map<String, Value>, CmdError> {
    let path = source.join("package.json");
    let bytes = std::fs::read(&path).map_err(|error| {
        CmdError::click(format!(
            "cannot read {}: {error}. A web product is a Node package; without its package.json there is no build script, no start script and no version to check",
            path.display()
        ))
    })?;
    let parsed: Value = serde_json::from_slice(&bytes).map_err(|error| {
        CmdError::click(format!("{} is not valid JSON: {error}", path.display()))
    })?;
    match parsed {
        Value::Object(map) => Ok(map),
        _ => Err(CmdError::click(format!(
            "{} is not a JSON object",
            path.display()
        ))),
    }
}

/// `package.json` when the checkout carries one.
///
/// A static site does not have to be a Node package at all: four of these
/// landing sites are `index.html` and a stylesheet, and Vercel served them by
/// copying the directory. Requiring a package manifest of them would be Stado
/// inventing a dependency the product does not have.
pub(in crate::cli::web::builds) fn manifest_if_present(
    source: &Path,
) -> Result<Option<Map<String, Value>>, CmdError> {
    if !source.join("package.json").is_file() {
        return Ok(None);
    }
    manifest(source).map(Some)
}

/// One npm script, if the product declares a non-empty one under that name.
pub(in crate::cli::web::builds) fn script<'a>(
    manifest: &'a Map<String, Value>,
    name: &str,
) -> Option<&'a str> {
    manifest
        .get("scripts")?
        .as_object()?
        .get(name)?
        .as_str()
        .filter(|body| !body.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_declared_non_empty_script_counts() {
        let manifest = serde_json::json!({
            "scripts": { "build": "next build", "lint": "   " }
        });
        let manifest = manifest.as_object().unwrap();
        assert_eq!(script(manifest, "build"), Some("next build"));
        // A script declared as whitespace runs nothing; treating it as present
        // would have the gate report a lint that never happened.
        assert_eq!(script(manifest, "lint"), None);
        assert_eq!(script(manifest, "typecheck"), None);
        assert_eq!(script(&Map::new(), "build"), None);
    }
}
