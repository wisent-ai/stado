//! The verdict: what the second read found, and the exit code it earns.

use crate::cli::CmdError;
use crate::deploy::service::{ImageIdentity, UnitImageObservation};

use super::settle::RESTART_WINDOW;

/// What the second read found.
///
/// Separated from the sentence and the exit code so the two branches that
/// cannot be reached without a real launchd restart — the one that worked and
/// the one that did not take effect — are still decided by tested logic rather
/// than by code nothing has ever executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshOutcome {
    /// The new process is executing the file the unit declares. The only
    /// success.
    OnDeclaredFile,
    /// Nothing is executing the unit's argument vector any more.
    NotRunning,
    /// A process is there and its identity could not be read.
    Unread,
    /// The new process is on the same image the old one was: launchd re-exec'd
    /// the declared path, and the path was never the problem.
    Unchanged,
    /// On some third file: neither the old image nor the declared one.
    StillWrong,
}

impl RefreshOutcome {
    /// Whether this outcome is the command succeeding.
    pub fn succeeded(self) -> bool {
        matches!(self, Self::OnDeclaredFile)
    }

    /// The one word a report names this outcome by.
    ///
    /// Crate-visible because the release agent's scheduled revisit pass
    /// records and prints these same five results, and a second set of words
    /// for them would be a second vocabulary an operator has to learn to
    /// compare a manual refresh with an automatic one. No caller outside the
    /// crate needs it.
    pub(crate) fn word(self) -> &'static str {
        match self {
            Self::OnDeclaredFile => "OnDeclaredFile",
            Self::NotRunning => "NotRunning",
            Self::Unread => "Unread",
            Self::Unchanged => "Unchanged",
            Self::StillWrong => "StillWrong",
        }
    }
}

/// Read the outcome out of the post-restart observation.
///
/// `was_running` is the image the unit held BEFORE the restart, which is the
/// only way to tell a restart that did nothing from one that landed somewhere
/// unexpected. Pure, and public so both post-restart branches are exercisable
/// without a launchd unit to point at.
pub fn refresh_outcome(
    was_running: &ImageIdentity,
    after: Option<&UnitImageObservation>,
) -> RefreshOutcome {
    let Some(after) = after else {
        return RefreshOutcome::NotRunning;
    };
    let (Some(running), Some(installed)) = (after.running.as_ref(), after.installed.as_ref())
    else {
        return RefreshOutcome::Unread;
    };
    if running.is_same_file(installed) {
        return RefreshOutcome::OnDeclaredFile;
    }
    if running.is_same_file(was_running) {
        return RefreshOutcome::Unchanged;
    }
    RefreshOutcome::StillWrong
}

/// The exit code and the sentence, decided by what the second read found and
/// never by the fact that a restart was issued.
pub(super) fn verdict(
    before: &UnitImageObservation,
    after: Option<&UnitImageObservation>,
    was_running: &ImageIdentity,
    installed: &ImageIdentity,
) -> Result<(), CmdError> {
    let outcome = refresh_outcome(was_running, after);
    if outcome.succeeded() {
        return Ok(());
    }
    let unit = &before.unit;
    let running = after
        .and_then(|row| row.running.as_ref())
        .map_or_else(|| "an unread image".to_string(), ImageIdentity::describe);
    let declared = after
        .and_then(|row| row.installed.as_ref())
        .unwrap_or(installed);
    Err(CmdError::click(match outcome {
        RefreshOutcome::OnDeclaredFile => unreachable!("handled above"),
        RefreshOutcome::NotRunning => format!(
            "{unit} was restarted and nothing is executing its argument vector {}s later. The \
             unit is now not running at all, which is worse than the stale image it was on: \
             check `stado service status {unit}`",
            RESTART_WINDOW.as_secs()
        ),
        RefreshOutcome::Unread => format!(
            "{unit} was restarted and the result could not be read, so whether it is on the \
             installed file is unknown. That is not the same as fixed"
        ),
        RefreshOutcome::Unchanged => format!(
            "{unit} was restarted and the restart did not take effect: the new process is \
             executing the same {running} it was on before. launchd re-execs the declared path \
             and the path was never the problem — pid 49727 did exactly this on 2026-09-03. The \
             declared file at {} is {}",
            declared.path,
            declared.describe()
        ),
        RefreshOutcome::StillWrong => format!(
            "{unit} was restarted and is executing {running}, which is still not the file it \
             declares at {} ({}). Something replaced the file again between the restart and this \
             read, or the unit reaches its program through a path that resolves elsewhere",
            declared.path,
            declared.describe()
        ),
    }))
}
