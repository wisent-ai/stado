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
    // A recipe that declares PATH runs with it; otherwise a step runs with
    // Stado's and the toolchains' homes ahead of the inherited PATH. Its
    // program is looked up on that same PATH.
    let path = match environment.get("PATH") {
        Some(declared) => std::ffi::OsString::from(declared),
        None => {
            let inherited = std::env::var_os("PATH")
                .context("PATH is missing; release steps need it to find their tools")?;
            step_search_path(None, Some(&inherited))?
        }
    };
    let mut command = Command::new(step_program(program, Some(&path))?);
    command
        .args(arguments)
        .envs(environment)
        .env("PATH", &path)
        .current_dir(root);
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
        atomic_json(&path, &Value::Array(steps))?;
    }
    Ok(())
}

/// A stable source for this revision, under its exact lockfile set, and one
/// Cargo target directory for every revision of the checkout on this
/// platform: the shared target keeps every dependency from a registry
/// compiled once. Cargo names a workspace crate by its path inside the
/// workspace, so every revision's crates share their names in that target,
/// and it calls one fresh when none of its sources is newer than its last
/// compile. A revision committed before another revision's later compile
/// therefore kept that compile's build of every crate it changed (an
/// install of stado 460c2a55 compiled against a stado-product without the
/// field the revision added). Every source file is dated last
/// ([`date_last`]), so a revision's own crates are compiled again under
/// Cargo's lock on the target, as `stado host cargo` dates its delivered
/// tree.
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
    let reused = match node_type(&source)? {
        Some(kind) if kind.is_dir() => true,
        None => false,
        Some(_) => anyhow::bail!(
            "installation Cargo cache source {} is not a real directory; refusing to follow or replace it",
            source.display()
        ),
    };
    if reused {
        refresh(exported, &source)?;
        fs::remove_dir_all(exported)?;
    } else {
        fs::rename(exported, &source)?;
    }
    date_last(&source)?;
    #[cfg(unix)]
    std::os::unix::fs::symlink(&source, exported)?;
    #[cfg(not(unix))]
    anyhow::bail!("installation Cargo caches require symbolic links on this platform");
    let target = area.join("cargo").join(platform).join("target");
    let wrapper = crate::compiler_cache::ensure(&runtime.home)?;
    let report = json!({"lockfiles": locks, "lockfile_sha256": digest,
        "platform": platform, "source_revision": revision, "target": target,
        "source": source, "reused": reused, "compiler_cache": wrapper.path});
    atomic_json(&evidence.join("cargo-cache.json"), &report)?;
    crate::common::tag_cache(&directory, "stado installation Cargo cache")?;
    crate::common::tag_cache(&target, "stado installation Cargo target")?;
    eprintln!(
        "Cargo cache: {} the source of revision {revision} at {}; lockfiles {digest}; target shared by every revision at {}",
        if reused { "reusing" } else { "creating" },
        source.display(),
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
    // The revision's directory holds its source; the target beside it is
    // shared by every revision and is not this run's to hold.
    let source = Path::new(
        report["source"]
            .as_str()
            .context("Cargo cache receipt has no source")?,
    );
    let directory = source
        .parent()
        .context("Cargo cache source has no parent")?;
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
    if !crate::common::files::present(&path)? {
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

/// The latest time every platform's file times state: the last minute of
/// the year 9999, the bound `touch -t` states and the time `stado host
/// cargo` gives its delivered tree.
const LAST_MINUTE: std::time::Duration = std::time::Duration::from_secs(253_402_300_740);

/// Date every file under `directory` at [`LAST_MINUTE`], so no compile in a
/// shared target is ever newer than these sources. Symbolic links keep
/// their own times.
fn date_last(directory: &Path) -> Result<()> {
    let last = std::time::UNIX_EPOCH + LAST_MINUTE;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            date_last(&entry.path())?;
        } else if kind.is_file() {
            fs::File::open(entry.path())
                .and_then(|file| file.set_modified(last))
                .with_context(|| format!("dating {} last", entry.path().display()))?;
        }
    }
    Ok(())
}

/// Restore committed inputs without rewriting identical files ([`cache`]
/// then dates every file last).
/// The revision never changes within this source directory. Build products
/// remain in place, while any build step's edits to committed files are undone.
fn refresh(exported: &Path, source: &Path) -> Result<()> {
    for entry in fs::read_dir(exported)? {
        let entry = entry?;
        let destination = source.join(entry.file_name());
        let kind = entry.file_type()?;
        let retained = restore_type(&destination, &kind)?;
        if kind.is_dir() {
            if !retained {
                fs::create_dir(&destination)?;
            }
            refresh(&entry.path(), &destination)?;
        } else if kind.is_file() {
            let same = retained && sha256(&entry.path())? == sha256(&destination)?;
            if !same {
                fs::copy(entry.path(), &destination)?;
            }
            fs::set_permissions(&destination, entry.metadata()?.permissions())?;
        } else if kind.is_symlink() {
            let link = fs::read_link(entry.path())?;
            if retained && fs::read_link(&destination)? == link {
                continue;
            }
            if retained {
                fs::remove_file(&destination)?;
            }
            #[cfg(unix)]
            std::os::unix::fs::symlink(link, destination)?;
        } else {
            anyhow::bail!(
                "unsupported committed source node {}",
                entry.path().display()
            );
        }
    }
    Ok(())
}

fn node_type(path: &Path) -> Result<Option<fs::FileType>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata.file_type())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("reading cached node {}", path.display())),
    }
}

/// Restore the committed node type without traversing a build-created symlink.
fn restore_type(path: &Path, expected: &fs::FileType) -> Result<bool> {
    let Some(actual) = node_type(path)? else {
        return Ok(false);
    };
    if (actual.is_dir() && expected.is_dir())
        || (actual.is_file() && expected.is_file())
        || (actual.is_symlink() && expected.is_symlink())
    {
        return Ok(true);
    }
    if actual.is_dir() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(false)
}
