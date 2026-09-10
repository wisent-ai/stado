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
            let mut captures = expression.captures_iter(text);
            let first = captures
                .next()
                .and_then(|capture| capture.name("version"))
                .map(|value| value.as_str().to_string())
                .ok_or_else(|| {
                    format!("version source {} did not produce version", path.display())
                })?;
            if captures.next().is_some() {
                return Err(format!(
                    "version source {} produced more than one version",
                    path.display()
                ));
            }
            first
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
