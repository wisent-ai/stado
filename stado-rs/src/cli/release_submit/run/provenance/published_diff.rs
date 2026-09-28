//! What a release candidate adds to its product's published history, carried
//! into the source archive for a release build, which has no history.
//!
//! A product gate that judges only what a change introduces (text a public
//! repository publishes, for example) needs the diff between the candidate
//! and what was already published. In a checkout that is a `git diff`; in a
//! worker's unpacked archive there is no `.git`, so the gate could only
//! refuse. The snapshot therefore adds one file at `PUBLISHED_DIFF_PATH`:
//! the newest tag `origin` serves whose commit is an ancestor of the
//! candidate and is not the candidate itself, and the diff from that commit
//! to the candidate, with its digest, so a gate can refuse a record that was
//! edited or truncated. A candidate is never its own baseline: a tag on the
//! candidate commit would make the diff empty and the gate would judge
//! nothing.

use std::path::Path;

use serde_json::json;
use sha2::{Digest, Sha256};

use super::origin_tag_commits;
use crate::cli::release_submit::run::source::git_text;
use crate::cli::CmdError;

/// Where the record sits inside the source archive.
pub(crate) const PUBLISHED_DIFF_PATH: &str = ".wisent-provenance/published-diff.json";

/// Whether `ancestor` is reachable from `commit`, by `git merge-base
/// --is-ancestor`: status 0 is yes, 1 is no (including a tag on unrelated
/// history, which is simply not below the candidate), anything else is a
/// failure that refuses the snapshot.
fn is_ancestor(root: &Path, ancestor: &str, commit: &str) -> Result<bool, CmdError> {
    let answer = std::process::Command::new("git")
        .args(["merge-base", "--is-ancestor", ancestor, commit])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .current_dir(root)
        .output()?;
    match answer.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(CmdError::click(format!(
            "git merge-base --is-ancestor {ancestor} {commit} failed: {}",
            String::from_utf8_lossy(&answer.stderr).trim()
        ))),
    }
}

/// The newest published tag below `commit`, as `(tag, commit)`, or `None`
/// when `origin` serves no tag on the candidate's history.
fn baseline(root: &Path, commit: &str) -> Result<Option<(String, String)>, CmdError> {
    let mut newest: Option<(String, String)> = None;
    for (tag, sha) in origin_tag_commits(root)? {
        if sha == commit {
            continue;
        }
        git_text(root, &["cat-file", "-e", &format!("{sha}^{{commit}}")]).map_err(|_| {
            CmdError::click(format!(
                "origin serves tag {tag} at {sha}, which this checkout does not hold; \
                 fetch tags from origin and submit again, or the published diff would be \
                 measured against an older release"
            ))
        })?;
        if !is_ancestor(root, &sha, commit)? {
            continue;
        }
        let newer = match &newest {
            Some((_, current)) => current != &sha && is_ancestor(root, current, &sha)?,
            None => true,
        };
        if newer {
            newest = Some((tag, sha));
        }
    }
    Ok(newest)
}

/// The record to archive, or `None` when no published tag lies below the
/// candidate.
pub(crate) fn record(root: &Path, commit: &str) -> Result<Option<Vec<u8>>, CmdError> {
    let Some((tag, base)) = baseline(root, commit)? else {
        return Ok(None);
    };
    let diff = git_text(
        root,
        &["diff", "--unified=0", "--no-color", "--no-renames", &base, commit],
    )?;
    let names = git_text(root, &["diff", "--name-only", "--no-renames", &base, commit])?;
    let paths = names.lines().map(str::to_owned).collect::<Vec<_>>();
    let document = json!({
        "baseline_tag": tag,
        "baseline_commit": base,
        "source_commit": commit,
        "paths": paths,
        "sha256": hex::encode(Sha256::digest(diff.as_bytes())),
        "diff": diff,
    });
    Ok(Some(serde_json::to_vec_pretty(&document)?))
}
