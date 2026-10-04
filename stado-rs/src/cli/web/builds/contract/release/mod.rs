//! What `.wisent-release.json` declares about this product: the name its
//! artifact is staged under, the site root it is served from, and the
//! build-time variables its platform sets.

pub(in crate::cli::web::builds) mod env;
pub(in crate::cli::web::builds) mod site;

use std::path::Path;

use serde_json::{Map, Value};

use crate::cli::CmdError;

/// The artifact's product name: `package.json`'s `name` with any `@scope/`
/// prefix removed.
///
/// The scope cannot survive into a file name — `dist/@wisent/foo-web.tar.gz`
/// is a second directory the recipe's stage map does not name, so the file
/// would never be collected. Anything else with a path separator in it is
/// refused rather than sanitised, because a staged path is what the manifest
/// matches on and quietly rewriting it produces an artifact the recipe cannot
/// find.
fn product_name(package_name: &str) -> Result<&str, CmdError> {
    let bare = match package_name.strip_prefix('@') {
        Some(scoped) => match scoped.split_once('/') {
            Some((scope, name)) if !scope.is_empty() => name,
            _ => {
                return Err(CmdError::click(format!(
                    "package.json name `{package_name}` starts with @ but names no scope: expected `@scope/name`"
                )).stating(crate::primitives::failure::FailureCode::Config))
            }
        },
        None => package_name,
    };
    if bare.is_empty() {
        return Err(CmdError::click(
            "package.json declares an empty name: the staged artifact is named after it",
        )
        .stating(crate::primitives::failure::FailureCode::Config));
    }
    if bare.contains('/') || bare.contains('\\') || bare == "." || bare == ".." {
        return Err(CmdError::click(format!(
            "package.json name `{package_name}` is not usable as a file name: the staged artifact is named after it"
        )).stating(crate::primitives::failure::FailureCode::Config));
    }
    Ok(bare)
}

/// The product name of the checkout being built.
///
/// `.wisent-release.json`'s `product` first, and `package.json`'s `name` only
/// when the checkout carries no manifest. The two disagree in practice and the
/// manifest is the one that matters: `preferences-landing`'s `package.json` is
/// named `preferences`, so naming the artifact after the package staged
/// `dist/preferences-web.tar.gz` while the recipe's stage map named
/// `dist/preferences-landing-web.tar.gz` — a build that succeeds and collects
/// nothing, which is the worst shape a release step can have. The stage map
/// and this name are two statements about one file, so both are read from the
/// document the release pipeline itself parses.
pub(in crate::cli::web::builds) fn product(
    source: &Path,
    manifest: Option<&Map<String, Value>>,
) -> Result<String, CmdError> {
    let release_manifest = source.join(crate::release_pipeline::PRODUCT_MANIFEST);
    if let Ok(text) = std::fs::read_to_string(&release_manifest) {
        let declared: Value = serde_json::from_str(&text).map_err(|error| {
            CmdError::click(format!(
                "{} is not valid JSON: {error}",
                release_manifest.display()
            ))
            .stating(crate::primitives::failure::FailureCode::Config)
        })?;
        if let Some(name) = declared.get("product").and_then(Value::as_str) {
            return product_name(name).map(str::to_string);
        }
        return Err(CmdError::click(format!(
            "{} declares no product, and the staged artifact is named after it",
            release_manifest.display()
        ))
        .stating(crate::primitives::failure::FailureCode::Config));
    }
    let declared = manifest
        .and_then(|manifest| manifest.get("name"))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            CmdError::click(
                "the checkout carries no .wisent-release.json and package.json declares no name, so \
                 the staged artifact has nothing to be named after",
            )
            .stating(crate::primitives::failure::FailureCode::Config)
        })?;
    product_name(declared).map(str::to_string)
}
