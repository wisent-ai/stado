//! Baseline provenance, verified where git history exists and carried into
//! the source archive for the release build, which has none.
//!
//! A product's version gate compares its surface with a committed
//! `released-surface.json` whose `source` marker names the artifact it was
//! recovered from. When that marker is `git-archive:<tag>`, only a checkout
//! can prove the tag exists at `origin` and names the commit the checkout
//! resolves it to; the release build reads an archive of files. So the
//! snapshot verifies the tag here and adds one file to the archive, at
//! `PROVENANCE_PATH`, recording the tag, its commit and its tree as `origin`
//! serves them. A tag `origin` does not serve, or serves at another commit,
//! refuses the snapshot: an archive must not carry a baseline whose source
//! nobody can find.

use std::path::Path;

use serde_json::{json, Value};

use super::source::{committed_file, git_text};
use crate::cli::CmdError;

/// Where the verified record sits inside the source archive.
pub(crate) const PROVENANCE_PATH: &str = ".wisent-provenance/baseline.json";
const BASELINE: &str = "released-surface.json";
const TIER: &str = "git-archive:";
const REMOTE: &str = "origin";
const PEELED: &str = "^{}";

/// The tag a committed baseline names, when it names one.
fn baseline_tag(root: &Path, commit: &str) -> Result<Option<String>, CmdError> {
    let Ok(bytes) = committed_file(root, commit, BASELINE) else {
        return Ok(None);
    };
    let document: Value = serde_json::from_slice(&bytes)
        .map_err(|error| CmdError::click(format!("{BASELINE} at {commit} is not JSON: {error}")))?;
    let marker = document["source"]
        .as_str()
        .and_then(|source| source.split_whitespace().next())
        .unwrap_or_default();
    Ok(marker.strip_prefix(TIER).map(str::to_string))
}

/// The commit `origin` serves for `tag`: the peeled row of an annotated tag,
/// else the one row of a lightweight tag.
fn remote_commit(root: &Path, tag: &str) -> Result<String, CmdError> {
    let reference = format!("refs/tags/{tag}");
    let listing = git_text(
        root,
        &[
            "ls-remote",
            REMOTE,
            &reference,
            &format!("{reference}{PEELED}"),
        ],
    )?;
    let rows = listing
        .lines()
        .filter_map(|line| line.split_once(char::is_whitespace))
        .collect::<Vec<_>>();
    rows.iter()
        .find(|(_, name)| name.trim().ends_with(PEELED))
        .or_else(|| rows.first())
        .map(|(sha, _)| sha.to_string())
        .ok_or_else(|| {
            CmdError::click(format!(
                "{BASELINE} was recovered from tag {tag}, which {REMOTE} does not serve; \
                 regenerate the baseline from a published tag before releasing"
            ))
        })
}

/// The provenance record to archive, or `None` when the committed baseline
/// names no git tag.
pub(crate) fn record(root: &Path, commit: &str) -> Result<Option<Vec<u8>>, CmdError> {
    let Some(tag) = baseline_tag(root, commit)? else {
        return Ok(None);
    };
    let local = git_text(
        root,
        &[
            "rev-parse",
            "--verify",
            &format!("refs/tags/{tag}^{{commit}}"),
        ],
    )
    .map_err(|error| {
        CmdError::click(format!(
            "{BASELINE} names tag {tag}, which this checkout cannot resolve ({error}); \
                 fetch tags from {REMOTE} and submit again"
        ))
    })?;
    let local = local.trim().to_string();
    let remote = remote_commit(root, &tag)?;
    if remote != local {
        return Err(CmdError::click(format!(
            "tag {tag} is {local} here and {remote} at {REMOTE}; the baseline would describe a \
             tree nobody released"
        )));
    }
    let tree = git_text(root, &["rev-parse", &format!("{local}^{{tree}}")])?;
    let document = json!({
        "baseline": BASELINE,
        "tag": tag,
        "commit": local,
        "tree": tree.trim(),
        "verified_against": REMOTE,
        "source_commit": commit,
    });
    Ok(Some(serde_json::to_vec_pretty(&document)?))
}
