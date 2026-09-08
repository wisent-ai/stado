//! The most recent quarantines that all failed one named way, and how long
//! such a run has to be before counting alone stops the next candidate.

use chrono::{DateTime, Utc};

use crate::release_agent::state::records::{HostReleaseState, QuarantineRecord};
use crate::release_cause::QuarantineCause;

/// How many consecutive quarantines sharing one named cause count as a wall
/// rather than a bad attempt.
///
/// Chosen by measuring the live data, not by taste. Classifying all twenty
/// `brama` records on `charless-mac-mini` gives a longest run of one classified
/// cause of **two** -- `brama-0.2.42` and `brama-0.2.43`, four days apart, both
/// refused at capability redemption. Two is ordinary: a candidate fails,
/// someone changes something, the next candidate fails the same way because the
/// change was wrong. Refusing at two would block that loop on its first honest
/// iteration.
///
/// Three is therefore the smallest threshold that fires on nothing in a month
/// of real history -- it raises no refusal anywhere in those twenty records --
/// while catching the first step past the worst run the fleet has actually
/// produced. Calibrating it against the data rather than the anecdote matters:
/// the 2026-09-01 sequence looks like a run of three and is not one, because
/// 0.2.49, 0.2.50 and 0.2.51 wrote no failure line and cannot be named.
///
/// The reason no historical window trips this is that twelve of the twenty rows
/// are unclassified. The threshold is worth having anyway, because from here on
/// a cause is recorded at the moment of quarantine from the whole log, so runs
/// become visible instead of being invisible in a column that did not exist.
pub(crate) const REPEAT_CAUSE_LIMIT: usize = 3;

/// Is this product about to walk into a wall it has already walked into?
///
/// `None` when there is no such wall.
///
/// **The "has anything changed" test.** The most recent [`REPEAT_CAUSE_LIMIT`]
/// quarantines must all name the same classified cause, and that is the whole
/// test, because of what it takes for a record to leave this map. The agent
/// only ever adds; the one thing that removes an entry is
/// `stado release quarantine clear --digest ... --reason ...`, which is an
/// operator stating, on the audit trail beside this file, that something
/// changed. So a run that is still intact *is* the assertion that nothing has
/// changed, and it needs no extra state to record.
///
/// A new digest is deliberately not a change. That is the exact mistake the
/// incident made: new digest, new version number, same unserved credential, and
/// every rollout treated the new digest as a new situation.
///
/// Consecutive, not "the last three classified": an unclassified quarantine
/// between two members is a candidate that failed in a way this agent could not
/// match to the others, and claiming it as more of the same is precisely the
/// overreach the cause vocabulary exists to avoid. It breaks the run.
///
/// Three ways out, none of them new and none of them a bypass flag:
///
/// - clear any one of the run's digests, which is the audited override and
///   immediately shortens the run below the limit;
/// - promote a candidate that fails for a *different* cause, which breaks the
///   run on its own;
/// - fix the cause, after which nothing quarantines and the run stops growing.
///
/// [`QuarantineCause::Unclassified`] never triggers this. Twelve of the twenty
/// live records are unclassified, seven of them consecutively, and refusing on a
/// cause the agent could not name would have frozen this product for a month on
/// no evidence at all.
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
    Some(CauseRun {
        cause,
        evidence: run[0].evidence.clone(),
        since: run[run.len() - 1].quarantined_at,
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
/// The run is however long it actually is — one row is a run of one — because
/// the length is no longer the whole decision. It is the input to two different
/// questions: what does the cause's own condition say right now, and failing
/// that, is this repetitive enough to stop on.
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

    /// Would counting alone stop the next candidate?
    ///
    /// The unchanged fallback: [`REPEAT_CAUSE_LIMIT`] consecutive quarantines
    /// of one classified cause. This is what governs every cause with no
    /// checkable condition, and what governs a cause whose condition could not
    /// be reached.
    pub fn repeats(&self) -> bool {
        self.len() >= REPEAT_CAUSE_LIMIT
    }
}
