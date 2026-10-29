//! What a candidate must be to take a place in the queue, checked when it
//! is submitted, before any gate runs. Its own commits (past their merge
//! base with `main`) may change no more lines than its product's
//! `submit.max_changed_lines`, so a change is split before its review
//! rather than reviewed whole; and one the risk rules already call high
//! (`risk`, over those commits and the gates its manifest declares) must
//! name a defect, since its review before landing is scoped to one.

use std::path::Path;

use super::{gates, risk};
use crate::cli::change::git::{changed, git};
use crate::cli::CmdError;

/// Refuse `commit` of `checkout`, repairing `defects`, when it is too big
/// for its product, high risk with no defect, or its manifest names no
/// product the queue can plan.
pub async fn admit(
    checkout: &Path,
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
    let base = git(checkout, &["merge-base", commit, tip]).await?;
    let (paths, lines) = changed(checkout, &base, commit).await?;
    if lines > max {
        return Err(CmdError::refused(format!(
            "{commit} changes {lines} lines in {} files past its merge base {base} with main, \
             more than the {max} one change of {product} may (its catalog \
             submit.max_changed_lines); split it into changes of at most {max} lines and submit \
             each",
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
