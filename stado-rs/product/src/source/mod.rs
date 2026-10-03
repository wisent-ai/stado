mod index;
pub(crate) use index::WorkspaceIndex;
mod provenance;
pub use provenance::{export, snapshot, verify_unchanged};

use crate::common::{capture, checked, Runtime};
use anyhow::{bail, Context, Result};
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

/// The canonical checkout of a source an operation builds: `checkout`, and
/// for an installation or update ([`Runtime::create_checkouts`]) one created
/// when none exists.
pub fn required_checkout(runtime: &Runtime, repository: &str) -> Result<PathBuf> {
    match checkout(runtime, repository) {
        Err(error) if runtime.create_checkouts && error.is::<MissingCheckout>() => {
            create_checkout(runtime, repository)
        }
        found => found,
    }
}

/// Create the canonical checkout of `repository` where the workspace keeps it,
/// `<workspace>/<name>` on `main`, by cloning its GitHub origin.
///
/// An installation names the product whose source it builds. A host that
/// never held that source (a fleet host a service is installed on, or a fresh
/// machine) otherwise refuses with the checkout missing, and nothing else in
/// Stado would ever create it. A directory already at that path that is not
/// the checkout is left alone and refused.
fn create_checkout(runtime: &Runtime, repository: &str) -> Result<PathBuf> {
    let repository = repository.to_ascii_lowercase();
    let name = Path::new(&repository)
        .file_name()
        .and_then(|name| name.to_str())
        .context("repository has no canonical name")?;
    let path = runtime.workspace.join(name);
    let missing = MissingCheckout {
        repository: repository.clone(),
        workspace: runtime.workspace.clone(),
    };
    if path.symlink_metadata().is_ok() {
        bail!(
            "{missing}: {} exists and does not identify {repository}",
            path.display()
        );
    }
    std::fs::create_dir_all(&runtime.workspace)
        .with_context(|| format!("{missing}: creating {}", runtime.workspace.display()))?;
    let origin = format!("https://github.com/{repository}.git");
    let output = capture(
        Command::new("git")
            .args(["clone", "--quiet", "--branch", "main", &origin])
            .arg(&path)
            .env("GIT_TERMINAL_PROMPT", "0"),
    )?;
    if !output.status.success() {
        bail!(
            "{missing}: cloning {origin} into {} failed ({}): {}",
            path.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    eprintln!(
        "{repository}: no canonical checkout was in {}; cloned {origin} into {}",
        runtime.workspace.display(),
        path.display()
    );
    validate(&path, Some(&repository))
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
            "--",
            ".",
            // Stado's own evidence and scratch inside the checkout are not
            // source; see provenance::capture.
            ":(top,exclude).wisent-output",
            ":(top,exclude).build",
        ],
    )?
    .is_empty()
    {
        revision.push_str("-dirty");
    }
    Ok(revision)
}

/// `commit`, as a full lowercase id, once the checkout holds it and
/// `origin/main` carries it. A commit origin/main does not carry is not
/// canonical source, whatever this checkout holds, and is refused.
pub fn canonical_commit(root: &Path, commit: &str) -> Result<String> {
    if commit.len() != 40
        || !commit
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        bail!("--source-commit requires a full lowercase Git commit");
    }
    let fetched = Command::new("git")
        .args(["fetch", "--quiet", "origin", "main"])
        .current_dir(root)
        .status()?;
    if !fetched.success() {
        bail!("fetching origin/main to prove {commit} canonical failed");
    }
    let carried = Command::new("git")
        .args(["merge-base", "--is-ancestor", commit, "origin/main"])
        .current_dir(root)
        .status()?;
    if !carried.success() {
        bail!("origin/main does not carry {commit}; only canonical source is installed");
    }
    Ok(commit.to_owned())
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
