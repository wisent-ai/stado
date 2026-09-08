//! Durable job records and their serialization: the typed transition
//! document, the workdir lifecycle verdict, the canonical digest keys and the
//! blob metadata projection of a job.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::models::Job;
use crate::queue::StorageError;

use super::JobStorage;

mod destination;
mod sentinel;
mod snapshot;

pub(in crate::queue::storage) use destination::merge_transition_destination;
pub(crate) use sentinel::is_transition_sentinel_state;
pub(in crate::queue::storage) use sentinel::{
    cleaned_transition_id, transition_cleaned_state, transition_fence_state,
    TRANSITION_CLEANED_PREFIX, TRANSITION_FENCE_PREFIX,
};
pub(crate) use snapshot::{validate_cancellation_snapshot, validate_transition_snapshot};

const TRANSITION_PREFIX: &str = "job-transitions";
pub(in crate::queue::storage) const TRANSITION_SCHEMA: &str = "stado.job-transition.v1";
pub(crate) const TRANSITION_RETIRED_STATE: &str = "retired:destination-verified-source-retired";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::queue::storage) struct JobTransition {
    pub(in crate::queue::storage) schema: String,
    pub(in crate::queue::storage) transition_id: String,
    pub(in crate::queue::storage) owner: String,
    pub(in crate::queue::storage) job_id: String,
    pub(in crate::queue::storage) from_prefix: String,
    pub(in crate::queue::storage) to_prefix: String,
    pub(in crate::queue::storage) source_version: String,
    pub(in crate::queue::storage) source_digest: String,
    pub(in crate::queue::storage) destination_version: Option<String>,
    pub(in crate::queue::storage) state: String,
    pub(in crate::queue::storage) created_at: String,
    pub(in crate::queue::storage) destination_job: Job,
}

/// Authoritative lifecycle verdict for one on-disk queue workdir candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkdirJobState {
    Live,
    Terminal,
    Unknown,
}

pub(in crate::queue::storage) fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub(crate) fn transition_path(job_id: &str) -> String {
    format!("{TRANSITION_PREFIX}/{}.json", sha256_hex(job_id.as_bytes()))
}

pub(crate) fn transition_is_retired(state: &str) -> bool {
    state == TRANSITION_RETIRED_STATE
}

pub(in crate::queue::storage) fn prefix_state(prefix: &str) -> &str {
    if prefix == "queue" {
        crate::models::job_state::QUEUED
    } else {
        prefix
    }
}

impl JobStorage {
    pub async fn refresh_job_metadata(&self, prefix: &str, job: &Job) -> Result<(), StorageError> {
        let blob_path = format!("{}/{}.json", prefix, job.job_id);
        self.backend
            .set_metadata(&blob_path, &Self::job_metadata(job))
            .await
    }

    pub(in crate::queue::storage) fn job_metadata(job: &Job) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("gpu_mem_gb".to_string(), job.gpu_mem_gb.to_string()),
            ("priority".to_string(), job.priority.to_string()),
            ("gpu_type".to_string(), job.gpu_type.clone()),
            ("provider".to_string(), job.provider.clone()),
            (
                "pin_to_provider".to_string(),
                job.pin_to_provider.to_string(),
            ),
        ])
    }
}
