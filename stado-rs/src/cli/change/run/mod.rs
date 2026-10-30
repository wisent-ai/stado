//! `stado change run [CHECKOUT]`: one pass of a repository's submit queue,
//! or of every repository this machine's queue holds a waiting candidate of.
//!
//! Under the repository's lock, every queued candidate, and every running
//! one whose pass is gone (killed, its machine restarted: the pid and birth
//! second it recorded name no live process), is taken oldest first and
//! carried to a verdict: its own commits written again on the remote
//! `main` ([`rebase`]), its declared gates run on the result ([`gates`]),
//! classified ([`risk`]), reviewed before landing when high risk
//! ([`review`]), pushed to `main` as a fast-forward with its landing
//! recorded first, so the repository's pre-push gate knows the push for
//! the queue's own, and recorded ([`integration`]). A `main` that moved
//! before the push takes the candidate back to the rebase, so what lands
//! is always exactly what the gates checked. A pass that cannot reach a
//! verdict because the remote, the fleet or the harsh review cannot be
//! run puts the candidate back in the queue with the step it waits on and
//! ends with that cause. One whose gates Oko's admission does not admit,
//! or does not answer for, is put back in the queue and held for the rest
//! of the pass, which carries the candidates behind it and then ends
//! naming it. One whose review before landing repaired it, or ended with no
//! round covering what its reviewer left, leaves the head as `repaired`,
//! and what the review left is queued at the tail ([`verdict`]), so no pass
//! runs the same review at the head again. A refused candidate is a
//! verdict, and the pass goes on. A landed candidate stays landed whatever
//! its review after landing meets.
//!
//! A pass that runs under no agent session is the queue's resident pass
//! (the host routine Stado's installation declares): its gates are asked of
//! Oko first as a run no session asked for, which the operator's consents
//! held to the fleet's daily build admit, and as the submitter's work only
//! when those do not ([`gates::Admission`]).

mod author;
pub(super) mod gates;
mod integration;
mod rebase;
mod review;
mod risk;
mod submission;
mod verdict;

use std::path::Path;

use chrono::Utc;

use crate::cli::change::git::{self, Pushed};
use crate::cli::change::record::{self, Candidate, Landed, State, WaitsOn};
use crate::cli::CmdError;
use rebase::Rewritten;
use verdict::{cancelled, follow_up, refuse, requeue};

pub(super) use submission::admit;

/// Run the queue of the repository whose checkout is `checkout`.
pub async fn run(
    checkout: &Path,
    hosts: &[String],
    integrated: Option<&Path>,
) -> Result<Vec<Candidate>, CmdError> {
    let repository = git::repository(checkout).await?;
    let Some(lock) = record::lock(&repository)? else {
        return Err(CmdError::refused(format!(
            "a pass of {repository}'s submit queue is already running; it takes every queued \
             candidate, so this one has nothing to do (`stado change list {}`)",
            checkout.display()
        )));
    };
    let resident = resident().await;
    pass(&repository, checkout, (hosts, integrated, resident), lock).await
}

/// Run the queue of every repository this machine's queue holds a
/// candidate waiting for a pass of, each from the checkout its oldest
/// waiting candidate was submitted from. A repository whose pass already
/// runs is left to it; one whose pass stops does not stop the others, and
/// every stop is named at the end.
pub async fn run_all(
    hosts: &[String],
    integrated: Option<&Path>,
) -> Result<Vec<Candidate>, CmdError> {
    let resident = resident().await;
    let mut repositories: Vec<(String, std::path::PathBuf)> = Vec::new();
    for candidate in record::list(None)? {
        if record::waits_for_pass(&candidate)
            && !repositories
                .iter()
                .any(|(repository, _)| *repository == candidate.repository)
        {
            repositories.push((candidate.repository.clone(), candidate.checkout.clone()));
        }
    }
    let mut carried = Vec::new();
    let mut stopped = Vec::new();
    for (repository, checkout) in repositories {
        let Some(lock) = record::lock(&repository)? else {
            eprintln!(
                "stado change run: a pass of {repository}'s submit queue is already running; \
                 its candidates are left to it"
            );
            continue;
        };
        match pass(&repository, &checkout, (hosts, integrated, resident), lock).await {
            Ok(done) => carried.extend(done),
            Err(error) => {
                eprintln!("stado change run: {repository}: {error}");
                stopped.push(format!("{repository}: {error}"));
            }
        }
    }
    if !stopped.is_empty() {
        return Err(CmdError::refused(format!(
            "the pass carried {} candidate(s) to a verdict, and the queue of {} repository(ies) \
             did not move: {}",
            carried.len(),
            stopped.len(),
            stopped.join("; ")
        )));
    }
    Ok(carried)
}

/// Whether this pass runs under no agent session, which makes it the
/// queue's resident pass ([`gates::Admission`]). A session register that
/// cannot be read is said, and the pass asks as the submitters' work.
async fn resident() -> bool {
    match crate::cli::release_loop::allowed::calling_session().await {
        Ok(None) => true,
        Ok(Some(session)) => {
            eprintln!(
                "stado change run: this pass runs inside agent session {session}, so each \
                 candidate's gates are asked of Oko as its submitter's work only"
            );
            false
        }
        Err(cause) => {
            eprintln!(
                "stado change run: the agent session this pass runs under could not be read \
                 ({cause}), so each candidate's gates are asked of Oko as its submitter's work \
                 only"
            );
            false
        }
    }
}

/// One pass of `repository`'s queue from `checkout`, under `_lock`, the
/// repository's pass lock this process holds until the pass ends.
async fn pass(
    repository: &str,
    checkout: &Path,
    (hosts, integrated, resident): (&[String], Option<&Path>, bool),
    _lock: std::fs::File,
) -> Result<Vec<Candidate>, CmdError> {
    let mut carried = Vec::new();
    // A candidate whose gates Oko's admission does not admit for its
    // submitter (a ruling, a closed defect), or does not answer for, is held
    // for this pass, by id with why: its admission is asked as its own
    // submitter's, so the candidates behind it are asked on their own.
    let mut held: Vec<(String, String)> = Vec::new();
    // Under the lock no other pass runs, so a running candidate is one a
    // pass that was stopped (killed, its machine restarted) left behind:
    // taken again, or it would stay running with no pass to end it.
    while let Some(mut candidate) = record::list(Some(repository))?
        .into_iter()
        .filter(|candidate| !held.iter().any(|(id, _)| *id == candidate.id))
        .find(|candidate| matches!(candidate.state, State::Queued | State::Running))
    {
        if candidate.state == State::Running {
            eprintln!(
                "stado change run: {} was left running by a pass that is gone; this pass resumes \
                 it",
                candidate.id
            );
        }
        candidate.state = State::Running;
        candidate.refusal = None;
        candidate.waits_on = None;
        candidate.pass = Some(record::this_pass());
        record::save(&mut candidate)?;
        eprintln!(
            "stado change run: {} ({}) of {repository}",
            candidate.id, candidate.commit
        );
        let landed = land(&mut candidate, checkout, hosts, integrated, resident).await;
        candidate.pass = None;
        if let Err(error) = landed {
            if candidate.state == State::Landed {
                // Its commit is on `main`: a step after the push that
                // failed is kept beside it, and it stays landed.
                candidate.waits_on = None;
                candidate
                    .unrecorded
                    .push(format!("the pass stopped after the landing: {error}"));
                record::save(&mut candidate)?;
                return Err(error.within(format!(
                    "{} landed; the pass of {repository} stopped after it",
                    candidate.id
                )));
            }
            let refused = error.failure == Some(crate::primitives::failure::FailureCode::Refused);
            // The gates step refuses only when Oko's admission did not
            // admit them, or did not answer.
            if refused && candidate.waits_on == Some(WaitsOn::Gate) {
                candidate.waits_on = Some(WaitsOn::Admission);
            }
            candidate.state = State::Queued;
            candidate.refusal = Some(format!("the pass stopped before a verdict: {error}"));
            record::save(&mut candidate)?;
            if refused {
                eprintln!("stado change run: {} stays queued: {error}", candidate.id);
                held.push((candidate.id.clone(), error.to_string()));
                continue;
            }
            return Err(error.within(format!(
                "{} stays queued; the pass of {repository} stopped",
                candidate.id
            )));
        }
        candidate.waits_on = None;
        record::save(&mut candidate)?;
        carried.push(candidate);
    }
    if !held.is_empty() {
        return Err(CmdError::refused(format!(
            "the pass of {repository} carried {} candidate(s) to a verdict and left {} queued, \
             each held by its own refusal: {}",
            carried.len(),
            held.len(),
            held.iter()
                .map(|(id, why)| format!("{id}: {why}"))
                .collect::<Vec<_>>()
                .join("; ")
        )));
    }
    Ok(carried)
}

/// Carry `candidate` to a verdict; `Err` when none could be reached, with
/// the step it stopped at in `waits_on`, or when its record could not be
/// kept after it landed (it is then landed).
async fn land(
    candidate: &mut Candidate,
    checkout: &Path,
    hosts: &[String],
    integrated: Option<&Path>,
    resident: bool,
) -> Result<(), CmdError> {
    let mut attempts = 0;
    let landed = loop {
        if cancelled(candidate)? {
            return Ok(());
        }
        attempts += 1;
        candidate.waits_on = Some(WaitsOn::Remote);
        let tip = git::fetch_main(checkout).await?;
        let rebased = match rebase::rebase(checkout, &candidate.commit, &tip).await? {
            Rewritten::Done(rebased) => rebased,
            Rewritten::AlreadyOnMain => {
                refuse(
                    candidate,
                    format!(
                        "main ({tip}) already holds {} or all it changes; nothing lands",
                        candidate.commit
                    ),
                );
                return Ok(());
            }
            Rewritten::Conflict(why) => {
                refuse(candidate, why);
                return Ok(());
            }
        };
        candidate.rebased = Some(rebased.clone());
        candidate.landing = None;
        record::save(candidate)?;
        let plan = match gates::plan(checkout, &rebased.commit, gates::QUEUED).await? {
            Ok(plan) => plan,
            Err(why) => {
                refuse(candidate, why);
                return Ok(());
            }
        };
        candidate.product = Some(plan.product.clone());
        let policy = match risk::policy(&plan.product) {
            Ok(policy) => policy,
            Err(error) => {
                refuse(candidate, error.to_string());
                return Ok(());
            }
        };
        let (paths, lines) = git::changed(checkout, &tip, &rebased.commit).await?;
        let classified = risk::classify(&plan.product, &policy, &paths, lines, &plan.names());
        eprintln!(
            "stado change run: {} is {} risk: {}",
            candidate.id,
            classified.level,
            classified.reasons.join("; ")
        );
        candidate.risk = Some(classified.clone());
        record::save(candidate)?;
        if cancelled(candidate)? {
            return Ok(());
        }
        candidate.waits_on = Some(WaitsOn::Gate);
        record::save(candidate)?;
        let purposes: Vec<&str> = candidate.defects.iter().map(String::as_str).collect();
        let admission = gates::Admission {
            purposes: &purposes,
            session: candidate.session.as_deref(),
            resident,
        };
        let outcome = gates::run(
            checkout,
            &rebased.commit,
            &rebased.tree,
            &plan,
            hosts,
            &admission,
        )
        .await?;
        candidate.shards = outcome.shards;
        record::save(candidate)?;
        if let Some(why) = outcome.refusal {
            refuse(candidate, why);
            return Ok(());
        }
        if classified.level == risk::HIGH {
            if cancelled(candidate)? {
                return Ok(());
            }
            candidate.waits_on = Some(WaitsOn::ReviewRound);
            record::save(candidate)?;
            let (review, repaired) =
                review::review(candidate, "before-land", &rebased.commit, &rebased.tree).await?;
            let verdict = review.verdict.clone();
            let said = review
                .answer
                .lines()
                .last()
                .unwrap_or("its reviewer gave no answer")
                .to_string();
            candidate.reviews.push(review);
            let why =
                format!("the harsh review before landing did not admit it ({verdict}: {said})");
            if verdict == review::REPAIRED || verdict == review::NO_ROUND {
                // Off the head: what the review left goes to the tail, so
                // the next pass carries the candidates behind it first.
                let left = repaired.unwrap_or_else(|| review::Repair {
                    commit: rebased.commit.clone(),
                    tree: rebased.tree.clone(),
                });
                requeue(candidate, left, why)?;
                return Ok(());
            }
            if verdict != review::ADMITTED {
                refuse(candidate, why);
                return Ok(());
            }
        }
        if cancelled(candidate)? {
            return Ok(());
        }
        // Recorded before the push, so the repository's pre-push gate finds
        // this exact commit as the queue's landing and does not run its
        // gates again.
        candidate.landing = Some(rebased.commit.clone());
        candidate.waits_on = Some(WaitsOn::Remote);
        record::save(candidate)?;
        match git::push(checkout, &rebased.commit).await? {
            Pushed::Landed => break rebased,
            Pushed::Moved => {
                eprintln!(
                    "stado change run: main moved past {tip}; {} is written again",
                    candidate.id
                );
            }
            Pushed::Refused(said) => {
                refuse(
                    candidate,
                    format!("the push of {} to main was refused: {said}", rebased.commit),
                );
                return Ok(());
            }
        }
    };
    eprintln!(
        "stado change run: {} landed as {}",
        candidate.id, landed.commit
    );
    candidate.state = State::Landed;
    candidate.landing = None;
    candidate.landed = Some(Landed {
        commit: landed.commit.clone(),
        at: Utc::now(),
        attempts,
    });
    record::save(candidate)?;
    integration::record(candidate, integrated).await;
    // Kept before the review after landing, which runs for as long as its
    // reviewer takes.
    record::save(candidate)?;
    if candidate
        .risk
        .as_ref()
        .is_some_and(|classified| classified.level == risk::LOW)
    {
        // The candidate is on `main` already: a review that cannot run, or
        // repairs that cannot be queued, are recorded beside it and never
        // put it back in the queue.
        match review::review(candidate, "after-land", &landed.commit, &landed.tree).await {
            Ok((review, repaired)) => {
                candidate.reviews.push(review);
                match follow_up(candidate, repaired) {
                    Ok(follow) if !follow.is_empty() => {
                        eprintln!("stado change run: {}{follow}", candidate.id);
                    }
                    Ok(_) => {}
                    Err(error) => candidate
                        .unrecorded
                        .push(format!("the reviewer's repairs were not queued: {error}")),
                }
            }
            Err(error) => {
                let not_run = review::not_run("after-land", &landed.tree, error.to_string());
                candidate.reviews.push(not_run);
            }
        }
    }
    Ok(())
}
