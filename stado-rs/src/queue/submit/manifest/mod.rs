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

/// Where one entry of a run manifest stands. The stored word is parsed into
/// this once by [`validate`]; a word outside it is an invalid manifest, and
/// every write stores [`EntryState::word`], so no state is spelled twice.
/// The queue's run documents (`queue::runs`) record terminal outcomes in
/// the same entries, so they read and write it through the same names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub(in crate::queue) enum EntryState {
    Planned,
    Claimed,
    Enqueuing,
    Accepted,
    /// The job reached a terminal prefix and its outcome is retained.
    Terminal,
    /// The job's outcome was retained after its transition was reaped.
    Reaped,
}

impl EntryState {
    /// The entry's job has ended and carries a retained outcome.
    pub(in crate::queue::submit) fn ended(self) -> bool {
        matches!(self, Self::Terminal | Self::Reaped)
    }

    /// The state as a manifest stores it.
    pub(in crate::queue) fn word(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Claimed => "claimed",
            Self::Enqueuing => "enqueuing",
            Self::Accepted => "accepted",
            Self::Terminal => "terminal",
            Self::Reaped => "reaped",
        }
    }

    /// The state a stored entry names; an entry naming no state this Stado
    /// reads is an invalid manifest, refused with the word it holds.
    pub(in crate::queue::submit) fn stored(
        entry: &serde_json::Map<String, Value>,
    ) -> Result<Self, SubmitError> {
        let state = entry.get("state").ok_or_else(|| {
            SubmitError::Validation("run checkpoint entry names no state".into())
        })?;
        serde_json::from_value(state.clone()).map_err(|error| {
            SubmitError::Validation(format!(
                "run checkpoint entry state {state} is no state of a run entry: {error}"
            ))
        })
    }
}

#[derive(Debug)]
pub(in crate::queue::submit) struct ManifestEntry {
    planned_job: Job,
    state: EntryState,
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
    validate_run_manifest(manifest, run_id, request, &request_digest).map(|_validated_entries| ())
}
