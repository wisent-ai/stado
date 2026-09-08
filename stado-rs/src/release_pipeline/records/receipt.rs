//! What a builder hands back: the step-by-step record of one platform build.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::release_pipeline::validate::predicates::default_extract;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Passed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepReceipt {
    pub name: String,
    pub argv: Vec<String>,
    pub status: StepStatus,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactReceipt {
    pub sha256: String,
    pub bytes: u64,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildReceipt {
    pub schema_version: u32,
    pub run_id: String,
    pub job_id: String,
    pub product: String,
    pub version: String,
    pub platform: String,
    pub builder: String,
    pub source_commit: String,
    pub source_sha256: String,
    pub manifest_sha256: String,
    #[serde(default)]
    pub inputs: BTreeMap<String, ReceiptInput>,
    #[serde(default)]
    pub secret_env: BTreeMap<String, String>,
    pub quality: Vec<StepReceipt>,
    pub build: StepReceipt,
    pub status: StepStatus,
    #[serde(default)]
    pub artifact: Option<ArtifactReceipt>,
    pub completed_at: String,
    #[serde(default)]
    pub failure: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptInput {
    pub uri: String,
    pub sha256: String,
    pub mount: String,
    #[serde(default = "default_extract")]
    pub extract: bool,
}
