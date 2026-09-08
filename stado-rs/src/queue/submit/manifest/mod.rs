//! Durable persistence of one submission: the `runs/<run_id>.json` manifest.
//!
//! [`validate`] proves a stored manifest is exactly the plan its request
//! derives, [`migrate`] upgrades a v2 manifest in place, and [`claim`] fences
//! and checkpoints each entry while it is being enqueued.

use serde_json::Value;

use crate::models::Job;

use super::{digest_value, SubmitError};

mod claim;
mod migrate;
mod validate;

pub(crate) use migrate::migrate_v2_run_manifest;

pub(in crate::queue::submit) use claim::{
    checkpoint_accepted, claim_entry, EntryClaim, SubmissionContext,
};
pub(in crate::queue::submit) use validate::validate_run_manifest;

#[derive(Debug)]
pub(in crate::queue::submit) struct ManifestEntry {
    planned_job: Job,
    state: String,
    outcome_job: Option<Job>,
}

pub(crate) fn validate_stored_run_manifest(
    manifest: &Value,
    run_id: &str,
) -> Result<(), SubmitError> {
    let request = manifest
        .get("request")
        .ok_or_else(|| SubmitError::Validation("run manifest request is missing".into()))?;
    let request_digest = digest_value(request);
    validate_run_manifest(manifest, run_id, request, &request_digest).map(|_| ())
}
