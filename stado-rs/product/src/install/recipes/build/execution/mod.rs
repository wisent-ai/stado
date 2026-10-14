use crate::common::{atomic_json, checked, runs, sha256, step_program, step_search_path, Runtime};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, path::Path, process::Command};

pub(super) fn step(
    step: &Value,
    root: &Path,
    environment: &BTreeMap<String, String>,
) -> Result<()> {
    let argv = step["argv"]
        .as_array()
        .context("release command requires argv")?
        .iter()
        .map(|value| value.as_str().context("release argv must contain strings"))
        .collect::<Result<Vec<_>>>()?;
    let (program, arguments) = argv.split_first().context("release argv is empty")?;
    let mut command = Command::new(step_program(program));
    command.args(arguments).envs(environment).current_dir(root);
    if !environment.contains_key("PATH") {
        let inherited = std::env::var_os("PATH")
            .context("PATH is missing; release steps need it to find their tools")?;
        if let Some(path) = step_search_path(None, &inherited) {
            command.env("PATH", path);
        }
    }
    let output = checked(&mut command)?;
    if let Some(directory) = environment.get("WISENT_INSTALL_EVIDENCE_DIR") {
        let path = Path::new(directory).join("build-steps.json");
        let mut steps: Vec<Value> = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
        };
        steps.push(
            json!({"argv": argv, "cwd": root, "exit_status": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr)}),
        );
        atomic_json(&path, &steps)?;
    }
    Ok(())
}

/// A stable source and target for this revision, under its exact lockfile set.
/// Other revisions never hold the same Cargo build-directory lock. Kache
/// shares compiler outputs across their isolated targets; Cargo itself keeps
/// the same revision fresh across installations, including workspace crates.
pub(super) fn cache(
    runtime: &Runtime,
    area: &Path,
    exported: &Path,
    revision: &str,
    recipe_key: &str,
    platform: &str,
    evidence: &Path,
) -> Result<Option<runs::Run>> {
    let mut locks = BTreeMap::new();
    lockfiles(exported, exported, &mut locks)?;
    if locks.is_empty() {
        return Ok(None);
    }
    let digest = hex::encode(Sha256::digest(serde_json::to_vec(&locks)?));
    let directory = area
        .join("cargo")
        .join(platform)
        .join(&digest)
        .join(revision)
        .join(recipe_key);
    let held = runs::hold(&directory)
        .with_context(|| format!("holding installation Cargo cache {}", directory.display()))?;
    let source = directory.join("source");
    let reused = source.is_dir();
    if reused {
        refresh(exported, &source)?;
        fs::remove_dir_all(exported)?;
    } else {
        fs::rename(exported, &source)?;
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(&source, exported)?;
    #[cfg(not(unix))]
    anyhow::bail!("installation Cargo caches require symbolic links on this platform");
    let target = directory.join("target");
    let wrapper = crate::compiler_cache::ensure(&runtime.home)?;
    let report = json!({"lockfiles": locks, "lockfile_sha256": digest,
        "platform": platform, "source_revision": revision, "target": target,
        "source": source, "reused": reused, "compiler_cache": wrapper.path});
    atomic_json(&evidence.join("cargo-cache.json"), &report)?;
    crate::common::tag_cache(&directory, "stado installation Cargo cache")?;
    eprintln!(
        "Cargo cache: {} at {}; lockfiles {digest}; revision {revision}",
        if reused { "reusing" } else { "creating" },
        target.display()
    );
    Ok(Some(held))
}

pub(super) fn resume(evidence: &Path) -> Result<Option<runs::Run>> {
    let path = evidence.join("cargo-cache.json");
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    let report: Value = serde_json::from_slice(&bytes)?;
    let target = Path::new(
        report["target"]
            .as_str()
            .context("Cargo cache receipt has no target")?,
    );
    let directory = target
        .parent()
        .context("Cargo cache target has no parent")?;
    Ok(Some(runs::hold(directory)?))
}

pub(super) fn environment(
    evidence: &Path,
    environment: &mut BTreeMap<String, String>,
) -> Result<()> {
    environment.insert(
        "WISENT_INSTALL_EVIDENCE_DIR".to_owned(),
        evidence.to_string_lossy().into_owned(),
    );
    let path = evidence.join("cargo-cache.json");
    if !path.exists() {
        return Ok(());
    }
    let report: Value = serde_json::from_slice(&fs::read(&path)?)?;
    for (variable, field) in [
        ("CARGO_TARGET_DIR", "target"),
        ("CARGO_BUILD_BUILD_DIR", "target"),
        ("RUSTC_WRAPPER", "compiler_cache"),
    ] {
        environment.insert(
            variable.to_owned(),
            report[field]
                .as_str()
                .with_context(|| format!("{} has no {field}", path.display()))?
                .to_owned(),
        );
    }
    Ok(())
}

fn lockfiles(root: &Path, path: &Path, locks: &mut BTreeMap<String, String>) -> Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            lockfiles(root, &entry.path(), locks)?;
        } else if kind.is_file() && entry.file_name() == "Cargo.lock" {
            locks.insert(
                entry
                    .path()
                    .strip_prefix(root)?
                    .to_string_lossy()
                    .into_owned(),
                sha256(&entry.path())?,
            );
        }
    }
    Ok(())
}

/// Restore committed inputs without changing the mtimes of identical files.
/// The revision never changes within this source directory. Build products
/// remain in place, while any build step's edits to committed files are undone.
fn refresh(exported: &Path, source: &Path) -> Result<()> {
    for entry in fs::read_dir(exported)? {
        let entry = entry?;
        let destination = source.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            fs::create_dir_all(&destination)?;
            refresh(&entry.path(), &destination)?;
        } else if kind.is_file() {
            let same = destination.is_file() && sha256(&entry.path())? == sha256(&destination)?;
            if !same {
                fs::copy(entry.path(), &destination)?;
            }
            fs::set_permissions(&destination, entry.metadata()?.permissions())?;
        } else if kind.is_symlink() {
            let link = fs::read_link(entry.path())?;
            if fs::read_link(&destination).ok().as_ref() != Some(&link) {
                if destination.symlink_metadata().is_ok() {
                    fs::remove_file(&destination)?;
                }
                #[cfg(unix)]
                std::os::unix::fs::symlink(link, destination)?;
            }
        }
    }
    Ok(())
}
