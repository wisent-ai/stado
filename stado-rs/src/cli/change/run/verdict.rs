//! How a candidate's pass ends a step: refused with the sentence it keeps,
//! withdrawn when `stado change cancel` asked for it, and the reviewer's
//! repairs queued as its follow-up.

use crate::cli::change::record::{self, Candidate, State};
use crate::cli::CmdError;

/// Refuse `candidate` with `why`, the sentence its record keeps.
pub(super) fn refuse(candidate: &mut Candidate, why: String) {
    eprintln!("stado change run: {} refused: {why}", candidate.id);
    candidate.state = State::Refused;
    candidate.refusal = Some(why);
}

/// Whether `stado change cancel` asked for `candidate` meanwhile.
pub(super) fn cancelled(candidate: &mut Candidate) -> Result<bool, CmdError> {
    if !record::cancel_requested(candidate)? {
        return Ok(false);
    }
    candidate.cancel_requested = true;
    candidate.state = State::Cancelled;
    Ok(true)
}

/// Queue the reviewer's repairs as a candidate of their own, carrying the
/// same defects: the sentence saying so, empty when it repaired nothing. A
/// follow-up's repairs are not queued again: they are named for the author
/// to read and submit, so the reviews of one change end.
pub(super) fn follow_up(
    candidate: &Candidate,
    repaired: Option<String>,
) -> Result<String, CmdError> {
    let Some(commit) = repaired else {
        return Ok(String::new());
    };
    if let Some(original) = &candidate.follow_up_of {
        // The repairs are committed on the reviewed commit, the candidate
        // written again on main, so the commit alone shows them.
        return Ok(format!(
            "; the reviewer repaired this follow-up of candidate {original} again, as commit \
             {commit}, which is not queued: read it (`git show {commit}`) and submit what \
             stands with `stado change submit`"
        ));
    }
    let queued = crate::cli::change::verbs::enqueue(
        &candidate.checkout,
        &candidate.repository,
        &commit,
        &candidate.defects,
        (candidate.session.clone(), Some(candidate.id.clone())),
    )?;
    Ok(format!(
        "; the reviewer's repairs are commit {commit}, queued as candidate {}",
        queued.id
    ))
}
