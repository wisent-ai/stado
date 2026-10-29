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

use crate::cli::release_submit::run::checkout::{
    committed_file, committed_file_if_present, git_text,
};
use crate::cli::CmdError;

mod published_diff;

/// Where the verified record sits inside the source archive.
pub(crate) const PROVENANCE_PATH: &str = ".wisent-provenance/baseline.json";
const BASELINE: &str = "released-surface.json";
const TIER: &str = "git-archive:";
const REMOTE: &str = "origin";
const PEELED: &str = "^{}";

/// Every history record a release build needs and cannot compute from files
/// alone, with the path it sits at in the source tree: the verified baseline
/// provenance and the published diff. A record that does not apply to this
/// commit is left out.
fn records(root: &Path, commit: &str) -> Result<Vec<(&'static str, Vec<u8>)>, CmdError> {
    let records = [
        (PROVENANCE_PATH, record(root, commit)?),
        (
            published_diff::PUBLISHED_DIFF_PATH,
            published_diff::record(root, commit)?,
        ),
    ];
    Ok(records
        .into_iter()
        .filter_map(|(path, record)| record.map(|record| (path, record)))
        .collect())
}

/// Add [`records`] to the release snapshot's archive.
pub(crate) fn append_records<W: std::io::Write>(
    files: &mut tar::Builder<W>,
    root: &Path,
    commit: &str,
) -> Result<(), CmdError> {
    for (path, record) in records(root, commit)? {
        let mut header = tar::Header::new_gnu();
        header.set_size(record.len() as u64);
        // The epoch and the root owner, so the archive's bytes depend on its
        // records alone.
        crate::primitives::archive::stamp(&mut header, crate::primitives::file_mode::shared_read());
        header.set_cksum();
        files.append_data(&mut header, path, &record[..])?;
    }
    Ok(())
}

/// Write [`records`] into `tree`, an exported copy of `commit`, so a check
/// over that tree reads the same history records the release build reads
/// from its archive: without them an application's version gate refused
/// every `stado quality check` for want of `.wisent-provenance/baseline.json`.
pub(crate) fn write_records(tree: &Path, root: &Path, commit: &str) -> Result<(), CmdError> {
    for (path, record) in records(root, commit)? {
        let target = tree.join(path);
        if let Some(folder) = target.parent() {
            std::fs::create_dir_all(folder)?;
        }
        std::fs::write(&target, &record)?;
    }
    Ok(())
}

/// The committed baseline's marker, when the commit holds a baseline. A
/// baseline git cannot read, or one naming no source, is the error: read as
/// no baseline, a release went out without the provenance its gate checks.
fn baseline_marker(root: &Path, commit: &str) -> Result<Option<String>, CmdError> {
    let Some(bytes) = committed_file_if_present(root, commit, BASELINE)? else {
        return Ok(None);
    };
    let document: Value = serde_json::from_slice(&bytes).map_err(|error| {
        CmdError::click(format!("{BASELINE} at {commit} is not JSON: {error}"))
            .stating(crate::primitives::failure::FailureCode::Config)
    })?;
    match document["source"]
        .as_str()
        .and_then(|source| source.split_whitespace().next())
    {
        Some(marker) => Ok(Some(marker.to_string())),
        None => Err(CmdError::declaration(format!(
            "{BASELINE} at {commit} names no source, so the baseline it records cannot be verified"
        ))),
    }
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
            CmdError::refused(format!(
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
    // Only a release manifest declares where a version is read from; a
    // tag of a product with another manifest kind, or an unreadable one,
    // carries no version and says which.
    let source = match serde_json::from_slice::<ProductManifest>(&manifest) {
        Ok(ProductManifest::Release(release)) => Ok(release.version_source),
        Ok(_other_manifest_kind) => {
            Err("the product manifest is not a release manifest".to_string())
        }
        Err(error) => Err(format!("the product manifest does not parse: {error}")),
    };
    let commits = origin_tag_commits(root)?;
    let mut tags = serde_json::Map::new();
    for (tag, sha) in commits {
        let declared = source.as_ref().map_err(Clone::clone).and_then(|source| {
            declared_version(source, |path| {
                committed_file(root, &sha, path).map_err(|error| error.to_string())
            })
        });
        let record = match declared {
            Ok(version) => json!({ "commit": sha, "version": version }),
            Err(unread) => {
                json!({ "commit": sha, "version": Value::Null, "version_unread": unread })
            }
        };
        tags.insert(tag, record);
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
        let mut wrapped = CmdError::click(format!(
            "{BASELINE} names tag {tag}, which this checkout cannot resolve ({error}); \
                 fetch tags from {REMOTE} and submit again"
        ));
        wrapped.failure = error.failure;
        wrapped
    })?;
    let local = local.trim().to_string();
    let remote = remote_commit(root, tag)?;
    if remote != local {
        return Err(CmdError::refused(format!(
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
