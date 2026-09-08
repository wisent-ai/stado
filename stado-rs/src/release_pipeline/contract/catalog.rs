//! The durable catalog entry that records one product's manifest as accepted.

use serde::{Deserialize, Serialize};

use super::manifest::ProductManifest;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogSourceIdentity {
    pub commit: String,
    pub source_sha256: String,
    pub source_uri: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseCatalogEntry {
    pub schema_version: u32,
    pub product: String,
    pub manifest_sha256: String,
    pub manifest: ProductManifest,
    #[serde(default)]
    pub source: Option<CatalogSourceIdentity>,
    pub recorded_at: String,
}
