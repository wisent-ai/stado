use crate::common::{git_command, Runtime};
use anyhow::{bail, Context, Result};
use stado_wait as wait;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

pub(crate) struct WorkspaceIndex {
    workspace: PathBuf,
    modified: SystemTime,
    repositories: BTreeMap<String, Vec<PathBuf>>,
    failures: BTreeMap<String, String>,
}

/// The repositories the checkouts directly under `workspace` declare, by
/// GitHub identity, and every checkout whose origin could not be read, by
/// its folder name with git's answer. Each checkout's origin is a few
/// milliseconds of `git config`; the whole scan is one wait, not one
/// recorded command per checkout, so a workspace of many checkouts reads as
/// one line, not hundreds.
type Origins = (BTreeMap<String, Vec<PathBuf>>, BTreeMap<String, String>);

fn origins(workspace: &Path) -> std::io::Result<Origins> {
    let mut repositories: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    let mut failures = BTreeMap::new();
    for entry in fs::read_dir(workspace)? {
        let entry = entry?;
        let path = entry.path();
        if !path.join(".git").exists() {
            continue;
        }
        // The declared origin, not `git remote get-url`: that answer has the
        // host's `url.<base>.insteadOf` rewrites applied, and a host that
        // rewrites GitHub to an SSH alias would see none of its checkouts.
        let output = git_command()?
            .args(["config", "--get", "remote.origin.url"])
            .current_dir(&path)
            .output()?;
        if output.status.success() {
            if let Some(identity) =
                super::repository(String::from_utf8_lossy(&output.stdout).trim())
            {
                repositories.entry(identity).or_default().push(path);
            }
        } else {
            failures.insert(
                entry.file_name().to_string_lossy().into_owned(),
                format!(
                    "reading origin in {}: {}: {}",
                    path.display(),
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            );
        }
    }
    Ok((repositories, failures))
}

pub fn candidates(runtime: &Runtime, repository: &str) -> Result<Vec<PathBuf>> {
    let modified = fs::metadata(&runtime.workspace)?.modified()?;
    let mut cache = runtime
        .checkouts
        .lock()
        .map_err(|_| anyhow::anyhow!("canonical repository index lock is poisoned"))?;
    if cache
        .as_ref()
        .is_none_or(|index| index.workspace != runtime.workspace || index.modified != modified)
    {
        let (repositories, failures) = wait::blocking(
            wait::Kind::Process,
            "git config --get remote.origin.url in every checkout of the workspace",
            runtime.workspace.display(),
            || origins(&runtime.workspace),
        )
        .with_context(|| {
            format!(
                "reading the origin of the checkouts in {}",
                runtime.workspace.display()
            )
        })?;
        *cache = Some(WorkspaceIndex {
            workspace: runtime.workspace.clone(),
            modified,
            repositories,
            failures,
        });
    }
    let index = cache
        .as_ref()
        .context("the canonical repository index was not built")?;
    let name = Path::new(repository)
        .file_name()
        .and_then(|name| name.to_str())
        .context("repository has no canonical name")?;
    if let Some(failure) = index.failures.get(name) {
        bail!("{failure}");
    }
    // A repository the workspace holds no checkout of has no candidates.
    Ok(match index.repositories.get(repository) {
        Some(found) => found.clone(),
        None => Vec::new(),
    })
}
