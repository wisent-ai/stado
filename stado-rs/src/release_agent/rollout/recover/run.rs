//! The latest consecutive quarantines sharing a classified cause.
//! Their recorded evidence, not their count, informs candidate admission.

use chrono::{DateTime, Utc};

use crate::release_agent::state::records::{HostReleaseState, QuarantineRecord};
use crate::release_cause::QuarantineCause;

/// Is this product about to walk into a wall it has already walked into?
///
/// `None` when there is no such wall.
///
/// The latest classified quarantine records a condition that has not been
/// cleared. A cause's live predicate may establish that it has gone; a new
/// version alone does not establish that a shared dependency has recovered.
/// An unclassified quarantine breaks the run rather than guessing its cause.
///
/// [`QuarantineCause::Unclassified`] never triggers this: an unnamed cause
/// cannot establish that consecutive candidates share the same failure.
pub fn cause_run(state: &HostReleaseState) -> Option<CauseRun> {
    let mut recent: Vec<&QuarantineRecord> = state.quarantined.values().collect();
    // The map is keyed by digest, so its own order is the digest's. Recency is
    // the question being asked.
    recent.sort_by_key(|record| std::cmp::Reverse(record.quarantined_at));
    let cause = recent.first()?.cause;
    if !cause.is_classified() {
        return None;
    }
    let run: Vec<&QuarantineRecord> = recent
        .into_iter()
        .take_while(|record| record.cause == cause)
        .collect();
    // The run holds at least the record the cause was read from: its newest
    // member carries the evidence, its oldest the time the run began.
    let (Some(newest), Some(oldest)) = (run.first(), run.last()) else {
        return None;
    };
    Some(CauseRun {
        cause,
        evidence: newest.evidence.clone(),
        since: oldest.quarantined_at,
        digests: state
            .quarantined
            .iter()
            .filter(|(_, record)| run.iter().any(|member| std::ptr::eq(*member, *record)))
            .map(|(digest, _)| digest.clone())
            .collect(),
    })
}

/// The most recent quarantines that all failed one named way.
///
/// The run carries every retained digest of the latest classified cause.
/// Its size is reported as evidence, not used as an admission threshold.
#[derive(Debug, Clone)]
pub struct CauseRun {
    pub cause: QuarantineCause,
    /// The decisive line from the most recent member of the run.
    pub evidence: String,
    /// When the oldest member of the run was quarantined.
    pub since: DateTime<Utc>,
    /// Every digest in the run, so the override names a real digest.
    pub digests: Vec<String>,
}

impl CauseRun {
    pub fn len(&self) -> usize {
        self.digests.len()
    }

    pub fn is_empty(&self) -> bool {
        self.digests.is_empty()
    }
}
