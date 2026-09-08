//! The `stado.submission-receipt.v3` document `stado submit` prints once the
//! durable submission returns: the run identity, the digests it is keyed by,
//! and one entry per submitted job.

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ResolvedExecutorReceipt {
    pub(super) provider: String,
    pub(super) machine_type: String,
    pub(super) gpu_type: String,
    pub(super) platform_os: String,
    pub(super) architecture: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SubmissionJobReceipt {
    pub(super) command_index: usize,
    pub(super) command: String,
    pub(super) command_digest: String,
    pub(super) job_key: String,
    pub(super) job_id: String,
    pub(super) output_uri: String,
    pub(super) pinned_host: String,
    pub(super) resolved_executor: ResolvedExecutorReceipt,
    pub(super) repo_ref: String,
    pub(super) submission_request_digest: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SubmissionReceipt {
    pub(super) schema: String,
    pub(super) run_id: String,
    pub(super) request_digest: String,
    pub(super) source_digest: String,
    pub(super) input_digest: String,
    pub(super) repo: String,
    pub(super) repo_ref: String,
    pub(super) source_revision: String,
    pub(super) jobs: Vec<SubmissionJobReceipt>,
}
