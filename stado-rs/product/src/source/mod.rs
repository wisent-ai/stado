mod index;
pub(crate) use index::WorkspaceIndex;
mod provenance;
pub use provenance::{snapshot, verify_unchanged};

use crate::common::{capture, checked, Runtime};
use anyhow::{bail, Result};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

pub fn git(root: &Path, args: &[&str]) -> Result<String> {
    let output = checked(Command::new("git").args(args).current_dir(root))?;
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

pub fn repository(remote: &str) -> Option<String> {
    let path = remote
        .strip_prefix("git@github.com:")
        .or_else(|| remote.strip_prefix("https://github.com/"))
        .or_else(|| remote.strip_prefix("ssh://git@github.com/"))?;
    let path = path.trim_end_matches('/').trim_end_matches(".git");
    let parts: Vec<_> = path.split('/').collect();
    if parts.len() != 2
        || parts
            .iter()
            .any(|p| p.is_empty() || *p == "." || *p == "..")
    {
        return None;
    }
    Some(path.to_ascii_lowercase())
}

pub fn validate(root: &Path, expected: Option<&str>) -> Result<PathBuf> {
    if root.symlink_metadata()?.file_type().is_symlink() {
        bail!("canonical checkout cannot be a symlink: {}", root.display());
    }
    let root = root.canonicalize()?;
    if !root.join(".git").is_dir() {
        bail!(
            "{} is not a canonical checkout; linked worktrees and source exports are refused",
            root.display()
        );
    }
    if Path::new(&git(&root, &["rev-parse", "--show-toplevel"])?).canonicalize()? != root {
        bail!("{} is nested below another checkout", root.display());
    }
    if git(&root, &["branch", "--show-current"])? != "main" {
        bail!("{} is not on main; no branch was switched", root.display());
    }
    if let Some(expected) = expected {
        let actual = repository(&git(&root, &["remote", "get-url", "origin"])?);
        if actual.as_deref() != Some(expected.to_ascii_lowercase().as_str()) {
            bail!(
                "{} does not identify {expected} through its GitHub origin",
                root.display()
            );
        }
    }
    Ok(root)
}

#[derive(Debug)]
pub struct MissingCheckout {
    pub repository: String,
    pub workspace: PathBuf,
}
impl std::fmt::Display for MissingCheckout {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "no canonical checkout answers for {} in {}; no checkout was created",
            self.repository,
            self.workspace.display()
        )
    }
}
impl std::error::Error for MissingCheckout {}

pub fn checkout(runtime: &Runtime, expected: &str) -> Result<PathBuf> {
    let expected = expected.to_ascii_lowercase();
    let matches = index::candidates(runtime, &expected)?;
    match matches.as_slice() {
        [path] => validate(path, Some(&expected)),
        [] => Err(MissingCheckout {
            repository: expected,
            workspace: runtime.workspace.clone(),
        }
        .into()),
        _ => bail!(
            "multiple checkouts answer for {expected}: {:?}; no checkout was selected",
            matches
        ),
    }
}

pub fn revision(root: &Path) -> Result<String> {
    let mut revision = git(root, &["rev-parse", "HEAD"])?;
    if !git(
        root,
        &[
            "--no-optional-locks",
            "status",
            "--porcelain",
            "--untracked-files=normal",
        ],
    )?
    .is_empty()
    {
        revision.push_str("-dirty");
    }
    Ok(revision)
}

pub fn advance(root: &Path, fetch: bool) -> Result<()> {
    validate(root, None)?;
    if fetch {
        git(root, &["fetch", "origin"])?;
    }
    if !git(root, &["status", "--porcelain", "--untracked-files=no"])?.is_empty() {
        bail!(
            "tracked edits hold {}; nothing was advanced",
            root.display()
        );
    }
    let ancestor = capture(
        Command::new("git")
            .args(["merge-base", "--is-ancestor", "HEAD", "origin/main"])
            .current_dir(root),
    )?;
    if !ancestor.status.success() {
        bail!(
            "{} has divergent history; nothing was advanced",
            root.display()
        );
    }
    git(root, &["merge", "--ff-only", "origin/main"])?;
    Ok(())
}
