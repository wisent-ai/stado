//! What a candidate must be to take a place in the queue, checked when it
//! is submitted, before any gate runs. Its own commits may change no more
//! lines than its product's `submit.max_changed_lines`, so a change is split
//! before its review rather than reviewed whole; and one the risk rules
//! already call high (`risk`, over those commits and the gates its manifest
//! declares) must name a defect, since its review before landing is scoped
//! to one. A candidate's own commits are those past its merge base with
//! `main`, or, for one stacked on a candidate still queued or running (its
//! commit holds that candidate's commit, which `main` does not), those past
//! the nearest such candidate: the commits below it are that candidate's,
//! measured when it was submitted and landed before this one.

use std::path::Path;

use super::{gates, risk};
use crate::cli::change::git::{changed, contains, git};
use crate::cli::change::record::{self, State};
use crate::cli::CmdError;

/// Refuse `commit` of `checkout` (repository `repository`), repairing
/// `defects`, when it is too big for its product, high risk with no defect,
/// or its manifest names no product the queue can plan.
pub async fn admit(
    checkout: &Path,
    repository: &str,
    commit: &str,
    tip: &str,
    defects: &[String],
) -> Result<(), CmdError> {
    let plan = gates::plan(checkout, commit, gates::QUEUED)
        .await?
        .map_err(CmdError::refused)?;
    let product = &plan.product;
    let policy = risk::policy(product)?;
    let Some(max) = policy.max_changed_lines else {
        return Err(CmdError::declaration(format!(
            "{product}'s submit declaration states no max_changed_lines, so the queue cannot \
             tell a change too big to review whole; declare the most lines one change of it may \
             change"
        )));
    };
    let (base, below) = match stacked_on(checkout, repository, commit, tip).await? {
        Some((id, parent)) => (parent, format!("past candidate {id} it is stacked on")),
        None => (
            git(checkout, &["merge-base", commit, tip]).await?,
            "past its merge base with main".to_string(),
        ),
    };
    let (paths, lines) = changed(checkout, &base, commit).await?;
    if lines > max {
        return Err(CmdError::refused(format!(
            "{commit} changes {lines} lines in {} files {below} ({base}), more than the {max} one \
             change of {product} may (its catalog submit.max_changed_lines); split it into \
             changes of at most {max} lines and submit each",
            paths.len()
        )));
    }
    let classified = risk::classify(product, &policy, &paths, lines, &plan.names());
    if classified.level == risk::HIGH && defects.is_empty() {
        return Err(CmdError::usage(format!(
            "{commit} is high risk ({}), so it is reviewed before it lands, and that review is \
             scoped to the defect it repairs; submit it with --defect ID",
            classified.reasons.join("; ")
        )));
    }
    Ok(())
}

/// The queued or running candidate of `repository` that `commit` is
/// stacked on, by id and commit: of those whose commit `commit` holds and
/// `main` (`tip`) does not, the nearest, the one no other such candidate's
/// commit sits above. None for a commit stacked on no waiting candidate.
async fn stacked_on(
    checkout: &Path,
    repository: &str,
    commit: &str,
    tip: &str,
) -> Result<Option<(String, String)>, CmdError> {
    let mut below: Vec<(String, String)> = Vec::new();
    for candidate in record::list(Some(repository))? {
        if !matches!(candidate.state, State::Queued | State::Running) || candidate.commit == commit
        {
            continue;
        }
        if contains(checkout, &candidate.commit, commit).await?
            && !contains(checkout, &candidate.commit, tip).await?
        {
            below.push((candidate.id, candidate.commit));
        }
    }
    let mut nearest: Option<(String, String)> = None;
    for (id, parent) in below {
        let above = match &nearest {
            None => true,
            Some((_, kept)) => contains(checkout, kept, &parent).await?,
        };
        if above {
            nearest = Some((id, parent));
        }
    }
    Ok(nearest)
}
