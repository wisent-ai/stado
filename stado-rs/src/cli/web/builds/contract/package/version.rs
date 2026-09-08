//! The version `package.json` states, and whether the release pipeline reads
//! the product's version there at all.

use std::path::Path;

use serde_json::{Map, Value};

use crate::cli::CmdError;

/// The version in `package.json`, refused when it disagrees with the version
/// the pipeline is cutting.
///
/// Asked only of a product whose `version_source` is that same field. The
/// check exists to catch a worker building a checkout other than the one the
/// release was cut from, and it can only do that by comparing two statements
/// about one version. `jeden` cuts its release from `Cargo.toml` and carries
/// a `package.json` that versions its npm wrapper separately — 0.1.0 against
/// the crate's 0.1.1 — so comparing them refused a correct checkout and said
/// the worker was building the wrong commit.
///
/// Whether the two are one statement is not a guess: the manifest says so,
/// in the `version_source` the release pipeline itself read to decide which
/// version is being cut.
pub(in crate::cli::web::builds) fn version_source_is_package_json(source: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(source.join(crate::release_pipeline::PRODUCT_MANIFEST))
    else {
        // No manifest, so `product` fell back to `package.json` for the name
        // and this falls back to it for the version: one document answering
        // both, which is the only consistent reading left.
        return true;
    };
    let Ok(declared) = serde_json::from_str::<Value>(&text) else {
        return true;
    };
    let Some(source) = declared.get("version_source") else {
        return true;
    };
    source.get("kind").and_then(Value::as_str) == Some("json")
        && source.get("path").and_then(Value::as_str) == Some("package.json")
        && source.get("pointer").and_then(Value::as_str) == Some("/version")
}

/// The version in `package.json` against the version being cut, for a product
/// where those are two statements about one thing.
pub(in crate::cli::web::builds) fn require_version(
    manifest: &Map<String, Value>,
    version: &str,
) -> Result<(), CmdError> {
    match manifest.get("version").and_then(Value::as_str) {
        Some(declared) if declared == version => Ok(()),
        Some(declared) => Err(CmdError::click(format!(
            "package.json declares version {declared} but WISENT_VERSION is {version}: the worker is not building the commit this release was cut from"
        ))),
        None => Err(CmdError::click(
            "package.json declares no version: the release pipeline reads the product's version from that field",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_version_must_match_the_release_being_cut() {
        let matching = serde_json::json!({ "version": "1.4.0" });
        require_version(matching.as_object().unwrap(), "1.4.0").unwrap();

        let drifted = serde_json::json!({ "version": "1.3.9" });
        let error = require_version(drifted.as_object().unwrap(), "1.4.0")
            .expect_err("a version mismatch must be refused");
        let message = error.message.unwrap_or_default();
        assert!(
            message.contains("1.3.9") && message.contains("1.4.0"),
            "{message}"
        );

        let absent = serde_json::json!({});
        require_version(absent.as_object().unwrap(), "1.4.0")
            .expect_err("a package.json with no version must be refused");
    }
}
