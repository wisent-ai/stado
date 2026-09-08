//! One recorded attempt against one unit, and the two sentences it can say
//! about itself.

use serde::{Deserialize, Serialize};

use crate::cli::service_refresh_image::RefreshOutcome;
use crate::deploy::service::ImageIdentity;

use super::identity::{AttemptOutcome, FileIdentity};

/// One attempt this host has recorded against one unit, and what came of it.
///
/// An attempt is recorded when the pass commits its intent, before any
/// restart is issued, so a row here says a restart was authorised and aimed —
/// not that `launchctl` ran. Only an `outcome` of
/// [`AttemptOutcome::Observed`] establishes that it did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevisitAttempt {
    /// The image the unit was executing when this attempt was recorded.
    pub was_running: FileIdentity,
    /// The file it declared then — what the attempt aimed it at.
    pub declared: FileIdentity,
    /// [`AttemptOutcome::word`].
    pub outcome: String,
    pub attempted_at: String,
    /// The launchd service target that answered, the refusal, or what the
    /// pass was about to do.
    pub service: String,
}

impl RevisitAttempt {
    /// Whether this record still bars another attempt.
    ///
    /// **How the state expires.** An attempt that did not land bars this unit
    /// only while BOTH identities are the ones it was made against, because
    /// that pair is the whole content of what it established: kickstarting
    /// this unit while it runs `was_running` and declares `declared` did not
    /// move it. Either side changing is a situation nothing has been tried in
    /// — the declared file replaced again (the common case at 0.13.50 to
    /// 0.14.8 in a day), or something else cycling the unit onto a third
    /// image, which is how `com.wisent.compute.agent.lukasz-macbook` healed
    /// itself. A wall clock is worse in both directions: too short
    /// re-kickstarts a unit launchd will not move, too long holds off a unit
    /// whose file changed an hour ago. The identity pair is not a proxy for
    /// "has anything changed" — it is that question.
    ///
    /// `Attempting` bars for the same reason and deliberately errs toward not
    /// acting. It says a restart was recorded as INTENDED against this pair
    /// and that no result was written — so whether launchd was ever asked is
    /// itself unknown. Issuing another restart is the one move that cannot be
    /// justified from that, because it is the move whose effect the record
    /// cannot rule out having already had.
    pub(crate) fn bars(&self, running: &ImageIdentity, declared: &ImageIdentity) -> bool {
        self.outcome != RefreshOutcome::OnDeclaredFile.word()
            && self.was_running.is(running)
            && self.declared.is(declared)
    }

    /// The clause `registry doctor` appends to the row that told an operator to
    /// restart this unit by hand.
    ///
    /// Outcome-specific, because one sentence cannot be true of all three
    /// shapes. Claiming "restarted and read the identity again" over a refusal
    /// or a lost result would be this feature reintroducing the defect it
    /// exists to remove, on the row an operator reads.
    pub(crate) fn clause(&self) -> String {
        let tail = "It will not be attempted again until the running image or the declared file \
                    changes";
        if self.outcome == AttemptOutcome::Attempting.word() {
            return format!(
                ". The release agent recorded its INTENT to restart this unit at {} and never \
                 recorded a result, so it stopped somewhere between committing that intent and \
                 writing down what happened. Whether `launchctl kickstart` was invoked at all is \
                 unknown: the intent is written first precisely so that a lost result cannot be \
                 mistaken for a restart that never happened, and it cannot say which of the two \
                 this was. {tail}",
                self.attempted_at
            );
        }
        if self.outcome == AttemptOutcome::RestartRefused.word() {
            return format!(
                ". The release agent tried to restart this unit at {} and launchd refused ({}); \
                 the identity was NOT re-read, so this row still describes the process that was \
                 running before. {tail}",
                self.attempted_at, self.service
            );
        }
        format!(
            ". The release agent restarted this unit at {} ({}) and read the identity again: {}. \
             {tail}",
            self.attempted_at, self.service, self.outcome
        )
    }
}
