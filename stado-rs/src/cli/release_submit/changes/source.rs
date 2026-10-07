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
        ))
        .stating(crate::primitives::failure::FailureCode::Config));
    }
    let text = String::from_utf8(output.stdout).map_err(|error| {
        CmdError::click(format!(
            "git {} in {} answered non-UTF-8: {error}",
            args.join(" "),
            root.display()
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown)
    })?;
    Ok(text.trim().to_owned())
}

pub(super) fn repository(root: &Path) -> Result<String, CmdError> {
    git(root, &["remote", "get-url", "origin"])
}

/// Whether this checkout holds `commit` as a commit object. A pending change
/// recorded against a commit its repository later lost (history rewritten on
/// origin) can never be an ancestor of anything this checkout releases.
pub(super) fn holds(root: &Path, commit: &str) -> Result<bool, CmdError> {
    let object = format!("{commit}^{{commit}}");
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["cat-file", "-e", &object])
        .output()?;
    Ok(output.status.success())
}

pub(crate) fn contains(root: &Path, older: &str, newer: &str) -> Result<bool, CmdError> {
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
        ))
        .stating(crate::primitives::failure::FailureCode::Config)),
    }
}

/// Oko's two kinds of assigned work, as its ids spell them: a transcript task
/// and a registered fleet defect (Oko's `defectEndPrefix`). A defect's repair
/// reaches a release the same way a task's does.
const WORK_ID_SHAPES: [(&str, usize); 2] = [("task-", 16), ("defect-", 8)];

fn is_work_id(work: &str) -> bool {
    WORK_ID_SHAPES.iter().any(|(prefix, digits)| {
        work.strip_prefix(prefix)
            .is_some_and(|hex| hex.len() == *digits && hex.bytes().all(|b| b.is_ascii_hexdigit()))
    })
}

pub(super) fn prepare(
    root: &Path,
    commit: &str,
    task: &str,
    session: &str,
) -> Result<Change, CmdError> {
    if !is_work_id(task) || session.trim().is_empty() {
        return Err(CmdError::refused(
            "a pending change requires an Oko work id (task-<16 hex digits> or defect-<8 hex digits>) and a session",
        ));
    }
    let root = root.canonicalize().map_err(|error| {
        CmdError::usage(format!(
            "--source {} is not a readable checkout: {error}; pass the product repository's checkout path",
            root.display()
        ))
    })?;
    // Every repository has one checkout and it works on main; a checkout left
    // on another branch is refused, naming the branch, so it is brought back
    // to main rather than worked around.
    let branch = git(&root, &["branch", "--show-current"])?;
    if branch != "main" {
        return Err(CmdError::refused(format!(
            "{} is on branch '{branch}', not main; the one checkout of a repository works on main, so return it to main (keeping any other session's edits) and submit again",
            root.display()
        )));
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
        .ok_or_else(|| CmdError::refused(format!("{repository} has no main branch on origin")))?;
    if !contains(&root, &commit, head)? {
        return Err(CmdError::refused(format!(
            "{commit} is not on {repository} origin/main ({head}); push it to main first, or fetch so this checkout holds origin's main"
        )));
    }
    let manifest = super::super::committed_file(&root, &commit, PRODUCT_MANIFEST)?;
    let ProductManifest::Release(manifest) =
        release_pipeline::parse_product_manifest(&manifest).map_err(CmdError::declaration)?
    else {
        return Err(CmdError::refused("product declares releases:false"));
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
