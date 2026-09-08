//! What a coordinator hands one builder for one platform of one run.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::release_pipeline::validate::predicates::default_extract;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerRequest {
    pub schema_version: u32,
    pub run_id: String,
    pub product: String,
    pub version: String,
    pub platform: String,
    pub builder: String,
    pub source_commit: String,
    pub source_sha256: String,
    pub manifest_sha256: String,
    pub source_archive: String,
    pub manifest_path: String,
    #[serde(default)]
    pub inputs: BTreeMap<String, WorkerInput>,
    #[serde(default)]
    pub secret_env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerInput {
    pub uri: String,
    pub sha256: String,
    pub archive_path: String,
    pub mount: String,
    #[serde(default = "default_extract")]
    pub extract: bool,
}
