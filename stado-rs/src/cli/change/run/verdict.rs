//! How a candidate's pass ends a step: refused with the sentence it keeps,
//! withdrawn when `stado change cancel` asked for it, taken off the head of
//! the queue with what its review left queued at the tail, and the
//! reviewer's repairs queued as its follow-up.

use super::review::Repair;
use crate::cli::change::record::{self, Candidate, Repairs, State};
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

/// Take `candidate` off the head of the queue after its review before
/// landing left `left`: the reviewer's repairs, or the reviewed commit
/// itself when it staged nothing and no round covers it. `left` is queued
/// at the tail as a candidate of its own, carrying the same defects, and
/// `candidate` is `repaired`, so the next pass does not run the same
/// review at the head again. A follow-up's own is not queued again: it is
/// named for its author and the follow-up refused, so the reviews of one
/// change end.
pub(super) fn requeue(
    candidate: &mut Candidate,
    left: Repair,
    why: String,
) -> Result<(), CmdError> {
    if let Some(original) = &candidate.follow_up_of {
        let commit = &left.commit;
        let why = format!(
            "{why}; it is the follow-up of candidate {original}, so commit {commit} its review \
             left is not queued again: read it (`git show {commit}`) and submit what stands with \
             `stado change submit`"
        );
        refuse(candidate, why);
        return Ok(());
    }
    let queued = enqueue_follow_up(candidate, &left.commit)?;
    eprintln!(
        "stado change run: {} repaired: {why}; {} is queued at the tail as candidate {}",
        candidate.id, left.commit, queued
    );
    candidate.state = State::Repaired;
    candidate.refusal = Some(format!(
        "{why}; commit {} is queued as candidate {queued}",
        left.commit
    ));
    candidate.repairs = Some(Repairs {
        commit: left.commit,
        tree: left.tree,
        queued_as: queued,
    });
    Ok(())
}

/// Queue the repairs of a review after landing as a candidate of their own,
/// carrying the same defects: the sentence saying so, empty when it
/// repaired nothing. A follow-up's repairs are not queued again: they are
/// named for the author to read and submit, so the reviews of one change
/// end.
pub(super) fn follow_up(
    candidate: &Candidate,
    repaired: Option<Repair>,
) -> Result<String, CmdError> {
    let Some(Repair { commit, .. }) = repaired else {
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
    let queued = enqueue_follow_up(candidate, &commit)?;
    Ok(format!(
        "; the reviewer's repairs are commit {commit}, queued as candidate {queued}"
    ))
}

/// Queue `commit` as the follow-up of `candidate`: its id.
fn enqueue_follow_up(candidate: &Candidate, commit: &str) -> Result<String, CmdError> {
    let queued = crate::cli::change::verbs::enqueue(
        &candidate.checkout,
        &candidate.repository,
        commit,
        &candidate.defects,
        (candidate.session.clone(), Some(candidate.id.clone())),
    )?;
    Ok(queued.id)
}
