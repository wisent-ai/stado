use super::state::Journal;
use crate::{common::capture, source::git};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

/// Where `repository`'s canonical checkout goes, refused when that path is
/// another request's or exists without this request having made it; the
/// flag says whether this request already claimed it. Reads only.
pub fn target(
    journal: Option<&Journal>,
    request: &str,
    repository: &str,
    workspace: &Path,
) -> Result<(PathBuf, bool)> {
    let name = repository
        .split_once('/')
        .context("repository requires OWNER/NAME")?
        .1;
    let path = workspace.join(name);
    let owner = match journal {
        Some(journal) => journal.get(&format!("checkout/{repository}"))?,
        None => None,
    };
    if owner.as_ref().is_some_and(|owner| owner != request) {
        bail!(
            "canonical checkout {} belongs to another creation request",
            path.display()
        );
    }
    if owner.is_none() && path.symlink_metadata().is_ok() {
        bail!(
            "canonical checkout already exists and was not created by this request: {}",
            path.display()
        );
    }
    Ok((path, owner.is_some()))
}

pub fn ensure(
    journal: &Journal,
    request: &str,
    repository: &str,
    workspace: &Path,
) -> Result<Value> {
    let (path, claimed) = target(Some(journal), request, repository, workspace)?;
    if !claimed {
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&path)?;
        journal.put(&format!("checkout/{repository}"), &json!(request))?;
    }
    if path.symlink_metadata()?.file_type().is_symlink()
        || path.canonicalize()?.parent() != Some(workspace)
    {
        bail!(
            "canonical checkout escapes its workspace: {}",
            path.display()
        );
    }
    if !path.join(".git").exists() {
        if fs::read_dir(&path)?.next().transpose()?.is_some() {
            bail!(
                "uninitialized checkout contains unowned files: {}",
                path.display()
            );
        }
        git(&path, &["init", "--initial-branch=main"])?;
    }
    if !path.join(".git").is_dir() {
        bail!("linked worktrees are not canonical creation targets");
    }
    let expected = format!("https://github.com/{repository}.git");
    if !git(&path, &["remote"])?
        .lines()
        .any(|remote| remote == "origin")
    {
        git(&path, &["remote", "add", "origin", &expected])?;
    }
    if git(&path, &["config", "--get", "remote.origin.url"])? != expected {
        bail!("canonical checkout origin differs from {repository}");
    }
    if git(&path, &["rev-parse", "--show-toplevel"])? != path.to_string_lossy() {
        bail!("creation path is not the repository root");
    }
    if git(&path, &["branch", "--show-current"])? != "main" {
        bail!("canonical checkout is not on main");
    }
    let head = capture(
        Command::new("git")
            .args(["rev-parse", "--verify", "HEAD"])
            .current_dir(&path),
    )?;
    if !head.status.success() {
        if !git(&path, &["status", "--porcelain"])?.is_empty() {
            bail!("incomplete checkout contains uncommitted files");
        }
        git(&path, &["fetch", "origin", "main"])?;
        git(&path, &["checkout", "-B", "main", "origin/main"])?;
    }
    if git(&path, &["worktree", "list", "--porcelain"])?
        .lines()
        .filter(|line| line.starts_with("worktree "))
        .count()
        != 1
    {
        bail!("repository has more than one checkout: {repository}");
    }
    Ok(
        json!({"repository": repository, "path": path, "revision": git(&path, &["rev-parse", "HEAD"])?}),
    )
}
