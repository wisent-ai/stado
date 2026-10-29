//! `stado change run CHECKOUT`: one pass of a repository's submit queue.
//!
//! Under the repository's lock, every queued candidate, and every one a
//! stopped pass left running, is taken oldest first and carried to a
//! verdict: its own commits written again on the remote `main`
//! ([`rebase`]), its declared gates run on the result ([`gates`]),
//! classified ([`risk`]), reviewed before landing when high risk
//! ([`review`]), pushed to `main` as a fast-forward, and recorded
//! ([`integration`]). A `main` that moved before the push takes the
//! candidate back to the rebase, so what lands is always exactly what the
//! gates checked. A pass that cannot reach a verdict because the remote,
//! the fleet or the harsh review does not answer puts the candidate back in
//! the queue and ends with that cause. One whose gates Oko's admission does
//! not admit, or does not answer for, is put back in the queue and held for
//! the rest of the pass, which carries the candidates behind it and then
//! ends naming it. A refused candidate is a verdict, and the pass goes on. A
//! landed candidate stays landed whatever its review after landing meets.

mod gates;
mod integration;
mod rebase;
mod review;
mod risk;
mod verdict;

use std::path::Path;

use chrono::Utc;

use crate::cli::change::git::{self, Pushed};
use crate::cli::change::record::{self, Candidate, Landed, State};
use crate::cli::CmdError;
use rebase::Rewritten;
use verdict::{cancelled, follow_up, refuse};

/// Run the queue of the repository whose checkout is `checkout`.
pub async fn run(
    checkout: &Path,
    hosts: &[String],
    integrated: Option<&Path>,
) -> Result<Vec<Candidate>, CmdError> {
    let repository = git::repository(checkout).await?;
    let Some(_lock) = record::lock(&repository)? else {
        return Err(CmdError::refused(format!(
            "a pass of {repository}'s submit queue is already running; it takes every queued \
             candidate, so this one has nothing to do (`stado change list {}`)",
            checkout.display()
        )));
    };
    let mut carried = Vec::new();
    // A candidate whose gates Oko's admission does not admit for its
    // submitter (a ruling, a closed defect), or does not answer for, is held
    // for this pass, by id with why: its admission is asked as its own
    // submitter's, so the candidates behind it are asked on their own.
    let mut held: Vec<(String, String)> = Vec::new();
    // Under the lock no other pass runs, so a running candidate is one a
    // pass that was stopped (killed, its machine restarted) left behind:
    // taken again, or it would stay running with no pass to end it.
    while let Some(mut candidate) = record::list(Some(&repository))?
        .into_iter()
        .filter(|candidate| !held.iter().any(|(id, _)| *id == candidate.id))
        .find(|candidate| matches!(candidate.state, State::Queued | State::Running))
    {
        candidate.state = State::Running;
        candidate.refusal = None;
        record::save(&mut candidate)?;
        eprintln!(
            "stado change run: {} ({}) of {repository}",
            candidate.id, candidate.commit
        );
        if let Err(error) = land(&mut candidate, checkout, hosts, integrated).await {
            if candidate.state == State::Landed {
                // Its commit is on `main`: a step after the push that
                // failed is kept beside it, and it stays landed.
                candidate
                    .unrecorded
                    .push(format!("the pass stopped after the landing: {error}"));
                record::save(&mut candidate)?;
                return Err(error.within(format!(
                    "{} landed; the pass of {repository} stopped after it",
                    candidate.id
                )));
            }
            candidate.state = State::Queued;
            candidate.refusal = Some(format!("the pass stopped before a verdict: {error}"));
            record::save(&mut candidate)?;
            if error.failure == Some(crate::primitives::failure::FailureCode::Refused) {
                eprintln!("stado change run: {} stays queued: {error}", candidate.id);
                held.push((candidate.id.clone(), error.to_string()));
                continue;
            }
            return Err(error.within(format!(
                "{} stays queued; the pass of {repository} stopped",
                candidate.id
            )));
        }
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

/// Carry `candidate` to a verdict; `Err` when none could be reached, or
/// when its record could not be kept after it landed (it is then landed).
async fn land(
    candidate: &mut Candidate,
    checkout: &Path,
    hosts: &[String],
    integrated: Option<&Path>,
) -> Result<(), CmdError> {
    let mut attempts = 0;
    let landed = loop {
        if cancelled(candidate)? {
            return Ok(());
        }
        attempts += 1;
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
        record::save(candidate)?;
        let plan = match gates::plan(checkout, &rebased.commit).await? {
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
        let purposes: Vec<&str> = candidate.defects.iter().map(String::as_str).collect();
        let outcome = gates::run(
            checkout,
            &rebased.commit,
            &rebased.tree,
            &plan,
            hosts,
            (&purposes, candidate.session.as_deref()),
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
            if verdict != review::ADMITTED {
                let follow = follow_up(candidate, repaired)?;
                refuse(
                    candidate,
                    format!(
                        "the harsh review before landing did not admit it ({verdict}: \
                         {said}){follow}"
                    ),
                );
                return Ok(());
            }
        }
        if cancelled(candidate)? {
            return Ok(());
        }
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
