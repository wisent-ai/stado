//! `stado change run` with no checkout: one pass of every repository this
//! machine's queue holds a candidate waiting for a pass of.
//!
//! Each repository's pass holds only that repository's lock, so the passes
//! run at once: a repository whose candidates wait on long harsh reviews no
//! longer holds every other repository's queue behind it for hours.

use std::path::{Path, PathBuf};

use futures::future::join_all;

use crate::cli::change::record::{self, Candidate};
use crate::cli::CmdError;

/// Run the queue of every repository this machine's queue holds a
/// candidate waiting for a pass of, each from the checkout its oldest
/// waiting candidate was submitted from, all at once, each under its own
/// lock. A repository whose pass already runs is left to it; one whose pass
/// stops does not stop the others, and every stop is named at the end.
pub async fn run_all(
    hosts: &[String],
    integrated: Option<&Path>,
) -> Result<Vec<Candidate>, CmdError> {
    let resident = super::resident().await;
    let mut repositories: Vec<(String, PathBuf)> = Vec::new();
    for candidate in record::list(None)? {
        if record::waits_for_pass(&candidate)
            && !repositories
                .iter()
                .any(|(repository, _)| *repository == candidate.repository)
        {
            repositories.push((candidate.repository.clone(), candidate.checkout.clone()));
        }
    }
    // Every lock is taken before any pass starts, so a repository whose pass
    // already runs is named at once rather than after the others finish.
    let mut locked = Vec::new();
    for (repository, checkout) in repositories {
        match record::lock(&repository)? {
            Some(lock) => locked.push((repository, checkout, lock)),
            None => eprintln!(
                "stado change run: a pass of {repository}'s submit queue is already running; \
                 its candidates are left to it"
            ),
        }
    }
    let passes = locked.into_iter().map(|(repository, checkout, lock)| async move {
        let outcome = super::pass(&repository, &checkout, (hosts, integrated, resident), lock).await;
        (repository, outcome)
    });
    let mut carried = Vec::new();
    let mut stopped = Vec::new();
    for (repository, outcome) in join_all(passes).await {
        match outcome {
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
