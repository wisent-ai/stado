use crate::common::{capture, Runtime};
use anyhow::{bail, Context, Result};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::SystemTime,
};

pub(crate) struct WorkspaceIndex {
    workspace: PathBuf,
    modified: SystemTime,
    repositories: BTreeMap<String, Vec<PathBuf>>,
    failures: BTreeMap<String, String>,
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
        let mut repositories: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
        let mut failures = BTreeMap::new();
        for entry in fs::read_dir(&runtime.workspace)
            .with_context(|| format!("reading workspace {}", runtime.workspace.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if !path.join(".git").exists() {
                continue;
            }
            let output = capture(
                Command::new("git")
                    .args(["remote", "get-url", "origin"])
                    .current_dir(&path),
            )?;
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
        *cache = Some(WorkspaceIndex {
            workspace: runtime.workspace.clone(),
            modified,
            repositories,
            failures,
        });
    }
    let index = cache.as_ref().unwrap();
    let name = Path::new(repository)
        .file_name()
        .and_then(|name| name.to_str())
        .context("repository has no canonical name")?;
    if let Some(failure) = index.failures.get(name) {
        bail!("{failure}");
    }
    Ok(index
        .repositories
        .get(repository)
        .cloned()
        .unwrap_or_default())
}
