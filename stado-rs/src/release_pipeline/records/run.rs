//! The durable record of one release run: its state, its per-platform builds
//! and its deliveries.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::release_pipeline::contract::recipe::PipelineChannel;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseRunState {
    Submitting,
    Waiting,
    Publishing,
    Delivering,
    Promoted,
    Reconciled,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformRunState {
    Submitted,
    Qualified,
    Published,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformRun {
    pub platform: String,
    pub builder: String,
    pub job_id: String,
    pub output_prefix: String,
    pub state: PlatformRunState,
    #[serde(default)]
    pub artifact_sha256: Option<String>,
    #[serde(default)]
    pub release_manifest_sha256: Option<String>,
    #[serde(default)]
    pub qualification_uri: Option<String>,
    #[serde(default)]
    pub failure: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryRunState {
    Submitted,
    Passed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryRun {
    pub name: String,
    pub platform: String,
    pub job_id: String,
    pub output_prefix: String,
    pub required: bool,
    pub state: DeliveryRunState,
    #[serde(default)]
    pub receipt_sha256: Option<String>,
    #[serde(default)]
    pub failure: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseRun {
    pub schema_version: u32,
    pub run_id: String,
    pub product: String,
    pub version: String,
    pub channel: PipelineChannel,
    pub source_commit: String,
    pub source_sha256: String,
    pub source_uri: String,
    pub manifest_sha256: String,
    pub manifest_uri: String,
    pub state: ReleaseRunState,
    pub platforms: BTreeMap<String, PlatformRun>,
    #[serde(default)]
    pub deliveries: BTreeMap<String, DeliveryRun>,
    #[serde(default)]
    pub failure: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}
