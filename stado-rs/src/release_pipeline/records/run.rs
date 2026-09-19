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
    /// A newer submission of the same product and channel replaced this run
    /// before it published: its queued builds were cancelled, a build already
    /// running is left to end and is not published. `failure` names the run.
    Superseded,
}

impl ReleaseRunState {
    /// The state a stored run names, or `None` for a word this product does
    /// not use. Readers of the run object spelled these states out again —
    /// `["promoted", "reconciled", "completed"]` in the web deploy stage,
    /// `"completed" | "failed" | "reconciled"` in the janitor, three more in
    /// the desktop — and a spelling nobody compiles is a spelling nobody
    /// renames.
    pub fn named(state: &str) -> Option<Self> {
        serde_json::from_value(serde_json::Value::String(state.to_owned())).ok()
    }

    /// The channel pointer has moved, so these bytes are installable.
    pub fn published(&self) -> bool {
        matches!(self, Self::Promoted | Self::Reconciled | Self::Completed)
    }

    /// The run has stopped moving: nothing further will be written to it.
    /// A superseded run is finished too, but it published nothing.
    pub fn finished(&self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Reconciled | Self::Failed | Self::Superseded
        )
    }

    /// Where the run stands, for a reader that only needs the three
    /// answers: it failed, it published, or it is still going.
    pub fn phase(&self) -> &'static str {
        match self {
            Self::Failed | Self::Superseded => "failed",
            state if state.published() => "published",
            _ => "in_flight",
        }
    }
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
