//! The signed manifest one release coordinate is published under, and the
//! qualification verdict it carries.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseManifest {
    pub schema_version: u32,
    pub product: String,
    pub version: String,
    pub platform: String,
    pub source_revision: String,
    pub source_sha256: String,
    pub pipeline_manifest_sha256: String,
    pub qualification_receipt_sha256: String,
    pub artifact_sha256: String,
    pub artifact_bytes: u64,
    pub binary: String,
    pub launcher: String,
    pub config_schema: u64,
    pub state_schema: u64,
    pub minimum_stado_version: String,
    #[serde(default)]
    pub rollback_compatible_with: Vec<String>,
    pub qualification: ReleaseQualification,
    pub key_id: String,
    pub built_at: String,
    pub builder: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseQualification {
    pub status: QualificationStatus,
    #[serde(default)]
    pub evidence_sha256: Option<String>,
    #[serde(default)]
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QualificationStatus {
    Pending,
    Passed,
    Failed,
}
