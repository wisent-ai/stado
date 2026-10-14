//! Reading the product version from a selected source, and holding a recorded
//! catalog entry to the manifest it claims to carry.

use std::path::Path;

use regex::Regex;
use serde_json::Value;

use crate::release_pipeline::contract::catalog::ReleaseCatalogEntry;
use crate::release_pipeline::contract::manifest::{ProductManifest, VersionSource};
use crate::release_pipeline::SCHEMA_VERSION;

use super::manifest::validate_product_manifest;
use super::predicates::{identifier, sha256};

pub fn declared_version(
    source: &VersionSource,
    read: impl FnOnce(&str) -> Result<Vec<u8>, String>,
) -> Result<String, String> {
    let path = Path::new(source.path());
    let bytes = read(source.path())?;
    let value = match source {
        VersionSource::Json { pointer, .. } => {
            let document: Value = serde_json::from_slice(&bytes).map_err(|error| {
                format!("version source {} is not JSON: {error}", path.display())
            })?;
            document
                .pointer(pointer)
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    format!(
                        "version source {} pointer {pointer:?} is not a string",
                        path.display()
                    )
                })?
                .to_string()
        }
        VersionSource::Regex { pattern, .. } => {
            let text = std::str::from_utf8(&bytes)
                .map_err(|_| format!("version source {} is not UTF-8", path.display()))?;
            let expression = Regex::new(pattern).map_err(|error| error.to_string())?;
            // A project that ships several targets (an app and its extension)
            // writes the version once per target, and those must agree, so the
            // same version repeated is one version; versions that differ are
            // refused by name, as the Tuist version gate refuses them.
            let mut found: Vec<String> = Vec::new();
            for capture in expression.captures_iter(text) {
                if let Some(value) = capture.name("version") {
                    if !found.iter().any(|seen| seen == value.as_str()) {
                        found.push(value.as_str().to_string());
                    }
                }
            }
            match found.as_slice() {
                [] => {
                    return Err(format!(
                        "version source {} did not produce version",
                        path.display()
                    ))
                }
                [one] => one.clone(),
                several => {
                    return Err(format!(
                        "version source {} produced more than one version: {}; every match \
                         must declare the same version",
                        path.display(),
                        several.join(", ")
                    ))
                }
            }
        }
        VersionSource::Text { .. } => std::str::from_utf8(&bytes)
            .map_err(|_| format!("version source {} is not UTF-8", path.display()))?
            .trim()
            .to_string(),
    };
    if !identifier(&value) {
        return Err(format!(
            "version source {} produced invalid coordinate {value:?}",
            path.display()
        ));
    }
    Ok(value)
}

/// The file a version source names, relative to the checkout.
pub fn version_path(source: &VersionSource) -> &str {
    source.path()
}

/// `bytes` of the file `source` names with the version it declares replaced
/// by `next`, everything else byte for byte: the one writer of a version, so
/// what [`declared_version`] reads back is `next`. A JSON source is changed
/// as text, at the one string literal holding the declared version; a file
/// holding it more than once is refused rather than guessed at.
pub fn with_version(source: &VersionSource, bytes: &[u8], next: &str) -> Result<Vec<u8>, String> {
    if !identifier(next) {
        return Err(format!("{next:?} is not a version coordinate"));
    }
    let current = declared_version(source, |_| Ok(bytes.to_vec()))?;
    let path = source.path();
    let text =
        std::str::from_utf8(bytes).map_err(|_| format!("version source {path} is not UTF-8"))?;
    let written = match source {
        VersionSource::Regex { pattern, .. } => {
            let expression = Regex::new(pattern).map_err(|error| error.to_string())?;
            expression
                .replace_all(text, |capture: &regex::Captures<'_>| {
                    let whole = capture.iter().next().flatten();
                    match (whole, capture.name("version")) {
                        (Some(whole), Some(value)) => format!(
                            "{}{next}{}",
                            &text[whole.start()..value.start()],
                            &text[value.end()..whole.end()]
                        ),
                        (Some(whole), None) => whole.as_str().to_string(),
                        (None, _) => String::new(),
                    }
                })
                .into_owned()
        }
        VersionSource::Json { .. } => {
            let literal = format!("\"{current}\"");
            let found: Vec<usize> = text.match_indices(&literal).map(|(at, _)| at).collect();
            let [only] = found.as_slice() else {
                return Err(format!(
                    "version source {path} holds {literal} {} times, so the declared one cannot be told apart",
                    found.len()
                ));
            };
            format!(
                "{}\"{next}\"{}",
                &text[..*only],
                &text[*only + literal.len()..]
            )
        }
        VersionSource::Text { .. } => {
            let start = text.len() - text.trim_start().len();
            let stop = text.trim_end().len();
            format!("{}{next}{}", &text[..start], &text[stop..])
        }
    };
    Ok(written.into_bytes())
}

pub fn validate_catalog_entry(entry: &ReleaseCatalogEntry) -> Result<(), String> {
    if entry.schema_version != SCHEMA_VERSION
        || !identifier(&entry.product)
        || !sha256(&entry.manifest_sha256)
    {
        return Err("release catalog entry identity is invalid".into());
    }
    validate_product_manifest(&entry.manifest)?;
    let manifest_product = match &entry.manifest {
        ProductManifest::Release(value) => &value.product,
        ProductManifest::NonRelease(value) => &value.product,
    };
    if manifest_product != &entry.product {
        return Err("release catalog product disagrees with its manifest".into());
    }
    if let Some(source) = &entry.source {
        if source.commit.len() != 40
            || !source.commit.bytes().all(|byte| byte.is_ascii_hexdigit())
            || !sha256(&source.source_sha256)
            || source.source_uri
                != format!(
                    "stado://sources/{}/{}/source.tar.gz",
                    entry.product, source.source_sha256
                )
        {
            return Err("release catalog source identity is invalid".into());
        }
    }
    Ok(())
}
