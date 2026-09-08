//! Redelivery: the separately serialized transaction that fences one named
//! delivery being run again from an exact completed release run.

pub(in crate::cli::release_submit) mod entry;
mod finish;
mod plan;
mod transaction;

use serde::{Deserialize, Serialize};

use crate::release_pipeline::ReleaseRunState;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum RedeliveryStage {
    IntentCreated,
    RunReopened,
    Submitted,
    Terminal,
    RunRestored,
    Completed,
    Failed,
}

impl RedeliveryStage {
    fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RedeliveryTransaction {
    schema_version: u32,
    retry_token_sha256: String,
    delivery: String,
    previous_run_state: ReleaseRunState,
    pinned_consumer: String,
    request_sha256: String,
    stage: RedeliveryStage,
    job_id: Option<String>,
    receipt_sha256: Option<String>,
    failure: Option<String>,
}
