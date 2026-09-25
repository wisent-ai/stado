use super::{git, repository, revision};
use crate::common::{atomic_json, atomic_write, checked};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{fs, path::Path, process::Command};

pub fn snapshot(root: &Path, evidence: &Path, scratch: &Path) -> Result<Value> {
    fs::create_dir_all(evidence)?;
    let report = capture(root, scratch, Some(&evidence.join("working.patch")))?;
    atomic_json(&evidence.join("source.json"), &report)?;
    Ok(report)
}

pub fn verify_unchanged(
    root: &Path,
    expected: &Value,
    evidence: &Path,
    scratch: &Path,
) -> Result<()> {
    let actual = capture(root, scratch, None)?;
    atomic_json(&evidence.join("source-after.json"), &actual)?;
    if &actual != expected {
        bail!(
            "source changed during the operation in {}; prepared artifacts were not accepted; before: {}; after: {}; evidence: {}",
            root.display(), expected, actual, evidence.display()
        );
    }
    Ok(())
}

fn capture(root: &Path, scratch: &Path, patch_path: Option<&Path>) -> Result<Value> {
    fs::create_dir_all(scratch)?;
    let revision = revision(root)?;
    let base = revision.trim_end_matches("-dirty");
    let identity = repository(&git(root, &["remote", "get-url", "origin"])?)
        .context("source has no canonical GitHub identity")?;
    let index = scratch.join(format!("index-{}", uuid::Uuid::new_v4()));
    let result: Result<Value> = (|| {
        checked(
            Command::new("git")
                .args(["read-tree", base])
                .env("GIT_INDEX_FILE", &index)
                .current_dir(root),
        )?;
        checked(
            Command::new("git")
                .args(["add", "--all", "--", "."])
                .env("GIT_INDEX_FILE", &index)
                .current_dir(root),
        )?;
        let tree = checked(
            Command::new("git")
                .arg("write-tree")
                .env("GIT_INDEX_FILE", &index)
                .current_dir(root),
        )?;
        let tree = String::from_utf8(tree.stdout)?.trim().to_owned();
        if let Some(path) = patch_path {
            let patch = checked(
                Command::new("git")
                    .args(["diff", "--cached", "--binary", "--full-index", base])
                    .env("GIT_INDEX_FILE", &index)
                    .current_dir(root),
            )?;
            atomic_write(path, &patch.stdout)?;
        }
        Ok(
            json!({"repository": identity, "repository_path": root, "revision": revision, "tree": tree}),
        )
    })();
    let cleanup = if index.exists() {
        fs::remove_file(&index)
            .with_context(|| format!("removing source index {}", index.display()))
    } else {
        Ok(())
    };
    match (result, cleanup) {
        (Err(error), Err(cleanup)) => {
            Err(error.context(format!("source index cleanup also failed: {cleanup:#}")))
        }
        (Err(error), _) => Err(error),
        (Ok(report), cleanup) => cleanup.map(|()| report),
    }
}
