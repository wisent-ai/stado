//! Baseline provenance, verified where git history exists and carried into
//! the source archive for the release build, which has none.
//!
//! A product's version gate compares its surface with a committed
//! `released-surface.json` whose `source` marker names the artifact it was
//! recovered from. When that marker is `git-archive:<tag>`, only a checkout
//! can prove the tag exists at `origin` and names the commit the checkout
//! resolves it to; the release build reads an archive of files. So the
//! snapshot adds one file to the archive, at `PROVENANCE_PATH`, recording the
//! marker and every tag `origin` serves (so the gate can tell whether a newer
//! tag than its baseline exists, or a `head:` baseline ignores one), and for
//! a `git-archive:` baseline the tag's commit and tree as `origin` serves
//! them. A tag `origin` does not serve, or serves at another commit, refuses
//! the snapshot: an archive must not carry a baseline whose source nobody can
//! find.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{json, Value};

use crate::cli::release_submit::run::source::{committed_file, git_text};
use crate::cli::CmdError;

mod published_diff;

/// Where the verified record sits inside the source archive.
pub(crate) const PROVENANCE_PATH: &str = ".wisent-provenance/baseline.json";
const BASELINE: &str = "released-surface.json";
const TIER: &str = "git-archive:";
const REMOTE: &str = "origin";
const PEELED: &str = "^{}";

/// Permission bits of a record added to a snapshot: owner read-write,
/// everyone else read, as `git archive` writes a regular file.
const RECORD_MODE: u32 = 0o644;

/// Add every history record a release build needs and cannot compute from
/// files alone: the verified baseline provenance and the published diff.
/// A record that does not apply to this commit is not written.
pub(crate) fn append_records<W: std::io::Write>(
    files: &mut tar::Builder<W>,
    root: &Path,
    commit: &str,
) -> Result<(), CmdError> {
    let records = [
        (PROVENANCE_PATH, record(root, commit)?),
        (
            published_diff::PUBLISHED_DIFF_PATH,
            published_diff::record(root, commit)?,
        ),
    ];
    for (path, record) in records {
        let Some(record) = record else {
            continue;
        };
        let mut header = tar::Header::new_gnu();
        header.set_size(record.len() as u64);
        header.set_mode(RECORD_MODE);
        header.set_mtime(0);
        header.set_cksum();
        files.append_data(&mut header, path, &record[..])?;
    }
    Ok(())
}

/// The committed baseline's marker, when the commit holds a baseline.
fn baseline_marker(root: &Path, commit: &str) -> Result<Option<String>, CmdError> {
    let Ok(bytes) = committed_file(root, commit, BASELINE) else {
        return Ok(None);
    };
    let document: Value = serde_json::from_slice(&bytes)
        .map_err(|error| CmdError::click(format!("{BASELINE} at {commit} is not JSON: {error}")))?;
    Ok(Some(
        document["source"]
            .as_str()
            .and_then(|source| source.split_whitespace().next())
            .unwrap_or_default()
            .to_string(),
    ))
}

/// Every tag name `origin` serves, sorted, so a gate without git history can
/// tell whether a newer published tag than its baseline exists, and whether a
/// `head:` baseline ignores one.
fn origin_tags(root: &Path) -> Result<Vec<String>, CmdError> {
    let listing = git_text(root, &["ls-remote", "--tags", REMOTE])?;
    let mut names = listing
        .lines()
        .filter_map(|line| line.split_once("refs/tags/"))
        .map(|(_, name)| name.trim().trim_end_matches(PEELED).to_string())
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    Ok(names)
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

/// The commit `origin` serves for every tag, from one listing: the peeled
/// row of an annotated tag, else the one row of a lightweight tag.
fn origin_tag_commits(root: &Path) -> Result<BTreeMap<String, String>, CmdError> {
    let listing = git_text(root, &["ls-remote", "--tags", REMOTE])?;
    let mut commits = BTreeMap::<String, String>::new();
    for (sha, reference) in listing
        .lines()
        .filter_map(|line| line.split_once(char::is_whitespace))
    {
        let Some(name) = reference.trim().strip_prefix("refs/tags/") else {
            continue;
        };
        match name.strip_suffix(PEELED) {
            Some(tag) => {
                commits.insert(tag.to_string(), sha.to_string());
            }
            None => {
                commits
                    .entry(name.to_string())
                    .or_insert_with(|| sha.to_string());
            }
        }
    }
    Ok(commits)
}

/// For every tag `origin` serves: the commit it serves (the peeled row of an
/// annotated tag) and the version that commit's tree declares, read with the
/// product's own `version_source` from the manifest at `commit`. A tag whose
/// tree cannot be read here, or declares no version in that source, carries
/// `null`, so a gate can tell "not established" from a version. This is what
/// lets a gate without git compare baselines by the versions the tagged
/// artifacts declare, not by how their tags are spelled: a floating tag such
/// as `v1` names no version, and a moved one names a different commit.
fn origin_tag_versions(root: &Path, commit: &str) -> Result<Value, CmdError> {
    use crate::release_pipeline::{declared_version, ProductManifest};

    let manifest = committed_file(root, commit, crate::release_pipeline::PRODUCT_MANIFEST)?;
    let source = match serde_json::from_slice::<ProductManifest>(&manifest) {
        Ok(ProductManifest::Release(release)) => Some(release.version_source),
        _ => None,
    };
    let commits = origin_tag_commits(root)?;
    let mut tags = serde_json::Map::new();
    for (tag, sha) in commits {
        let version = source.as_ref().and_then(|source| {
            declared_version(source, |path| {
                committed_file(root, &sha, path).map_err(|error| error.to_string())
            })
            .ok()
        });
        tags.insert(tag, json!({ "commit": sha, "version": version }));
    }
    Ok(Value::Object(tags))
}

/// The provenance record to archive, or `None` when the commit holds no
/// baseline. Every record lists the tags `origin` serves; a `git-archive:`
/// baseline's tag is also resolved here and must be served at the same commit.
pub(crate) fn record(root: &Path, commit: &str) -> Result<Option<Vec<u8>>, CmdError> {
    let Some(marker) = baseline_marker(root, commit)? else {
        return Ok(None);
    };
    let mut document = json!({
        "baseline": BASELINE,
        "marker": marker.clone(),
        "origin_tags": origin_tags(root)?,
        "verified_against": REMOTE,
        "source_commit": commit,
        "origin_tag_versions": origin_tag_versions(root, commit)?,
    });
    let Some(tag) = marker.strip_prefix(TIER) else {
        return Ok(Some(serde_json::to_vec_pretty(&document)?));
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
    let remote = remote_commit(root, tag)?;
    if remote != local {
        return Err(CmdError::click(format!(
            "tag {tag} is {local} here and {remote} at {REMOTE}; the baseline would describe a \
             tree nobody released"
        )));
    }
    let tree = git_text(root, &["rev-parse", &format!("{local}^{{tree}}")])?;
    document["tag"] = json!(tag);
    document["commit"] = json!(local);
    document["tree"] = json!(tree.trim());
    Ok(Some(serde_json::to_vec_pretty(&document)?))
}
