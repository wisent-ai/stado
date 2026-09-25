use crate::{
    common::{emit, Arguments, Runtime},
    signing, state,
};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

pub fn owners(runtime: &Runtime) -> Result<BTreeMap<PathBuf, String>> {
    let mut owners = BTreeMap::new();
    for state in state::all(runtime)? {
        if state.status == "absent" {
            continue;
        }
        for path in state.protected_paths() {
            let resolved = path.canonicalize().unwrap_or(path.clone());
            let owner = format!("{}/{}", state.product, state.surface);
            if let Some(previous) = owners.insert(resolved.clone(), owner.clone()) {
                if previous.split_once('/').map(|(product, _)| product)
                    != Some(state.product.as_str())
                {
                    anyhow::bail!(
                        "{} is recorded by both {previous} and {owner}",
                        resolved.display()
                    );
                }
            }
        }
    }
    Ok(owners)
}

pub fn executable(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

pub fn fault(paths: &[PathBuf], runtime: &Runtime) -> Result<Option<(String, bool)>> {
    let directories: Vec<_> =
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect();
    for path in paths {
        if path.parent() != Some(runtime.home.join(".local/bin").as_path())
            && path.parent() != Some(runtime.home.join(".stado/bin").as_path())
        {
            continue;
        }
        if !executable(path) {
            return Ok(Some((
                format!("{} is not executable", path.display()),
                true,
            )));
        }
        let name = path
            .file_name()
            .context("installed executable has no filename")?;
        let first = directories
            .iter()
            .map(|directory| directory.join(name))
            .find(|candidate| executable(candidate));
        let expected = path.canonicalize()?;
        match first {
            Some(candidate) if candidate.canonicalize()? == expected => {}
            Some(candidate) => {
                return Ok(Some((
                    format!(
                        "{} is shadowed on PATH by {}",
                        path.display(),
                        candidate.display()
                    ),
                    false,
                )))
            }
            None => return Ok(Some((format!("{} is not on PATH", path.display()), false))),
        }
    }
    Ok(None)
}

pub fn admit_name(destination: &Path, canonical: &Path, runtime: &Runtime) -> Result<()> {
    if destination.parent() != Some(runtime.home.join(".local/bin").as_path())
        || destination.symlink_metadata().is_ok()
    {
        return Ok(());
    }
    let expected = canonical
        .canonicalize()
        .unwrap_or_else(|_| canonical.to_path_buf());
    let name = destination
        .file_name()
        .context("installed executable has no filename")?;
    for directory in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        let candidate = directory.join(name);
        if executable(&candidate) && candidate.canonicalize()? != expected {
            anyhow::bail!(
                "{} would change the meaning of '{}' on PATH; it already runs {}",
                destination.display(),
                name.to_string_lossy(),
                candidate.display()
            );
        }
    }
    Ok(())
}

pub fn collisions(runtime: &Runtime) -> Result<Vec<Value>> {
    let owned = owners(runtime)?;
    let mut rows = Vec::new();
    let local = runtime.home.join(".local/bin");
    if !local.exists() {
        return Ok(rows);
    }
    let directories: Vec<_> =
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect();
    for entry in fs::read_dir(local)? {
        let entry = entry?;
        let path = entry.path();
        if !executable(&path) {
            continue;
        }
        let installed = path.canonicalize()?;
        let mut seen = BTreeSet::new();
        for directory in &directories {
            let candidate = directory.join(entry.file_name());
            if !executable(&candidate) {
                continue;
            }
            let resolved = candidate.canonicalize()?;
            if resolved == installed || !seen.insert(resolved) {
                continue;
            }
            rows.push(
                json!({"name": entry.file_name().to_string_lossy(), "installed": path,
                "foreign": candidate, "recorded": owned.get(&installed)}),
            );
        }
    }
    Ok(rows)
}

pub fn residue(roots: &[PathBuf], runtime: &Runtime) -> Result<Vec<Value>> {
    let owned = owners(runtime)?;
    let mut rows = Vec::new();
    let mut seen = BTreeSet::new();
    for root in roots {
        if !root.exists() {
            continue;
        }
        for entry in fs::read_dir(root)? {
            let path = entry?.path();
            if !path.is_file() || !signing::native(&path)? {
                continue;
            }
            let resolved = path.canonicalize()?;
            if !seen.insert(resolved.clone()) {
                continue;
            }
            let mut report = signing::inspect(&resolved)?;
            if report["state"] != "stable" && report["state"] != "not_applicable" {
                report["recorded"] = json!(owned.get(&resolved));
                rows.push(report);
            }
        }
    }
    Ok(rows)
}

pub fn run(args: clap::ArgMatches, runtime: &Runtime) -> Result<i32> {
    let args = Arguments::from_matches(args);
    if !args.positional.is_empty() {
        anyhow::bail!("paths does not take positional arguments");
    }
    emit(&json!({"collisions": collisions(runtime)?}))?;
    Ok(0)
}
