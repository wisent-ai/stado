use super::Change;
use crate::cli::CmdError;
use crate::release_pipeline::{self, ProductManifest, PRODUCT_MANIFEST};
use std::path::Path;
use std::process::Command;

fn git(root: &Path, args: &[&str]) -> Result<String, CmdError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()?;
    if !output.status.success() {
        return Err(CmdError::click(format!(
            "git {} in {}: {}",
            args.join(" "),
            root.display(),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(String::from_utf8(output.stdout)
        .map_err(super::failure)?
        .trim()
        .to_owned())
}

pub(super) fn repository(root: &Path) -> Result<String, CmdError> {
    git(root, &["remote", "get-url", "origin"])
}

pub(super) fn contains(root: &Path, older: &str, newer: &str) -> Result<bool, CmdError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["merge-base", "--is-ancestor", older, newer])
        .output()?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(CmdError::click(format!(
            "cannot prove release coverage: {}",
            String::from_utf8_lossy(&output.stderr)
        ))),
    }
}

pub(super) fn prepare(
    root: &Path,
    commit: &str,
    task: &str,
    session: &str,
) -> Result<Change, CmdError> {
    if task.len() != 21
        || !task.starts_with("task-")
        || !task[5..].bytes().all(|b| b.is_ascii_hexdigit())
        || session.trim().is_empty()
    {
        return Err(CmdError::click(
            "a pending change requires task-<16 hex digits> and a session",
        ));
    }
    let root = root.canonicalize()?;
    if git(&root, &["branch", "--show-current"])? != "main" {
        return Err(CmdError::click(
            "submit pushed changes from the canonical main checkout",
        ));
    }
    let commit = super::super::resolve_commit(&root, Some(commit))?;
    let repository = repository(&root)?;
    // Ask the remote, not a possibly stale origin/main tracking ref. No fetch,
    // checkout or build occurs. The remote head must already exist locally.
    let remote = git(
        &root,
        &["ls-remote", "--exit-code", "origin", "refs/heads/main"],
    )?;
    let head = remote
        .split_whitespace()
        .next()
        .ok_or_else(|| CmdError::click("origin has no main branch"))?;
    if !contains(&root, &commit, head)? {
        return Err(CmdError::click(
            "commit is not on the pushed origin/main history",
        ));
    }
    let manifest = super::super::committed_file(&root, &commit, PRODUCT_MANIFEST)?;
    let ProductManifest::Release(manifest) =
        release_pipeline::parse_product_manifest(&manifest).map_err(super::failure)?
    else {
        return Err(CmdError::click("product declares releases:false"));
    };
    let identity = serde_json::to_vec(&(&repository, &manifest.product, task, &commit))?;
    Ok(Change {
        id: crate::release_control::sha256_bytes(&identity),
        product: manifest.product,
        task_id: task.into(),
        session_id: session.into(),
        source_commit: commit,
        repository,
        submitted_at: chrono::Utc::now().to_rfc3339(),
    })
}
