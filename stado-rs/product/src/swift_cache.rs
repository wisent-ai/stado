//! SwiftPM inputs store artifact paths relative to the cache, never to a publisher's scratch.
use crate::common::{atomic_json, checked, lock, relative, sha256, unpack};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{fs, path::Path, process::Command};

const STATE: &str = "workspace-state.json";
const RECEIPT: &str = "stado-swiftpm-input.json";

fn read(path: &Path) -> Result<Value> {
    serde_json::from_slice(&fs::read(path).with_context(|| format!("reading {}", path.display()))?)
        .with_context(|| format!("decoding {}", path.display()))
}

/// Called before publication, while the producer's actual files still exist.
pub fn make_portable(scratch: &Path) -> Result<()> {
    let scratch = scratch.canonicalize()?;
    let state_path = scratch.join(STATE);
    let mut state = read(&state_path)?;
    for artifact in state["object"]["artifacts"]
        .as_array_mut()
        .context("Swift workspace state has no artifacts array")?
    {
        let path = Path::new(
            artifact["path"]
                .as_str()
                .context("Swift artifact has no path")?,
        );
        let portable = path.strip_prefix(&scratch).with_context(|| {
            format!(
                "Swift artifact {} is outside {}",
                path.display(),
                scratch.display()
            )
        })?;
        relative(portable)?;
        if !path.canonicalize()?.starts_with(&scratch) {
            bail!("Swift artifact escapes its cache: {}", path.display());
        }
        artifact["path"] = json!(portable);
    }
    atomic_json(&state_path, &state)
}

fn artifacts(state: &mut Value, staged: &Path, destination: &Path, portable: bool) -> Result<()> {
    for artifact in state["object"]["artifacts"]
        .as_array_mut()
        .context("Swift workspace state has no artifacts array")?
    {
        let path = Path::new(
            artifact["path"]
                .as_str()
                .context("Swift artifact has no path")?,
        );
        let member = if portable {
            relative(path).context(
                "Swift input contains publisher-local artifact paths; republish with --swiftpm",
            )?
        } else {
            path.strip_prefix(destination)
                .context("Restored Swift artifact is outside this cache")?
        };
        relative(member)?;
        let actual = staged.join(member).canonicalize().with_context(|| {
            format!(
                "Swift artifact is missing: {}",
                staged.join(member).display()
            )
        })?;
        if !actual.starts_with(staged) {
            bail!(
                "Swift artifact escapes its restored cache: {}",
                actual.display()
            );
        }
        artifact["path"] = json!(destination.join(member));
    }
    Ok(())
}

fn pins(state: &Value, package: &Path, scratch: &Path) -> Result<()> {
    let lock_path = package.join("Package.resolved");
    let locked = read(&lock_path)?;
    let locked = locked["pins"]
        .as_array()
        .context("Package.resolved has no pins array")?;
    let dependencies = state["object"]["dependencies"]
        .as_array()
        .context("Swift workspace state has no dependencies array")?;
    let tracked = dependencies
        .iter()
        .filter(|dependency| dependency["state"]["checkoutState"]["revision"].is_string())
        .count();
    if tracked != locked.len() {
        bail!(
            "Swift input dependency set differs from {}; republish with --swiftpm",
            lock_path.display()
        );
    }
    for pin in locked {
        let identity = pin["identity"]
            .as_str()
            .context("Swift pin has no identity")?;
        let revision = pin["state"]["revision"]
            .as_str()
            .context("Swift pin has no revision")?;
        let dependency = dependencies
            .iter()
            .find(|value| value["packageRef"]["identity"] == identity)
            .with_context(|| {
                format!("Swift input lacks pinned dependency {identity}; republish with --swiftpm")
            })?;
        if dependency["state"]["checkoutState"]["revision"] != revision {
            bail!(
                "Swift input revision for {identity} differs from {}; republish with --swiftpm",
                lock_path.display()
            );
        }
        let subpath = dependency["subpath"]
            .as_str()
            .context("Swift dependency has no checkout subpath")?;
        let checkout = scratch
            .join("checkouts")
            .join(relative(Path::new(subpath))?);
        let checkout = checkout
            .canonicalize()
            .with_context(|| format!("Swift checkout is unavailable: {}", checkout.display()))?;
        if !checkout.starts_with(scratch) {
            bail!("Swift checkout escapes its cache: {}", checkout.display());
        }
        let actual = checked(Command::new("git").arg("-C").arg(&checkout).args([
            "rev-parse",
            "--verify",
            "HEAD",
        ]))?;
        if String::from_utf8(actual.stdout)?.trim() != revision {
            bail!(
                "Swift checkout {} is not at pinned revision {revision}",
                checkout.display()
            );
        }
    }
    Ok(())
}

/// Restore into an empty dependency cache; a repeated matching restore is read-only.
/// Unrelated compiler outputs are retained, and existing unowned cache entries are refused.
pub fn restore(package: &Path, archive: &Path) -> Result<Value> {
    let package = package.canonicalize().with_context(|| {
        format!(
            "Swift package directory is unavailable: {}",
            package.display()
        )
    })?;
    let scratch = package.join(".build");
    fs::create_dir_all(&scratch)
        .with_context(|| format!("creating Swift cache {}", scratch.display()))?;
    if scratch.symlink_metadata()?.file_type().is_symlink() {
        bail!("Swift cache cannot be a symlink: {}", scratch.display());
    }
    let scratch = scratch.canonicalize()?;
    let _writer = lock(&scratch.join("stado-swiftpm.lock"))?;
    let digest = sha256(archive)?;
    let receipt_path = scratch.join(RECEIPT);
    if receipt_path.try_exists()? {
        let mut receipt = read(&receipt_path)?;
        if receipt["archive_sha256"] != digest {
            bail!(
                "{} belongs to another Swift input; use a fresh release source directory",
                scratch.display()
            );
        }
        let mut state = read(&scratch.join(STATE))?;
        pins(&state, &package, &scratch)?;
        artifacts(&mut state, &scratch, &scratch, false)?;
        receipt["reused"] = json!(true);
        return Ok(receipt);
    }
    let stage = scratch.join(format!(".restore-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&stage)
        .with_context(|| format!("creating Swift restore staging {}", stage.display()))?;
    let result = (|| -> Result<Value> {
        unpack(archive, &stage)
            .with_context(|| format!("extracting Swift input {}", archive.display()))?;
        let staged = stage.join(".build");
        for entry in fs::read_dir(&stage)? {
            let entry = entry?;
            if entry.file_name() != ".build" || !entry.file_type()?.is_dir() {
                bail!(
                    "Swift input has a member outside .build: {}",
                    entry.path().display()
                );
            }
        }
        let mut state = read(&staged.join(STATE))?;
        pins(&state, &package, &staged)?;
        artifacts(&mut state, &staged, &scratch, true)?;
        atomic_json(&staged.join(STATE), &state)?;
        let entries = fs::read_dir(&staged)?.collect::<std::io::Result<Vec<_>>>()?;
        for entry in &entries {
            let destination = scratch.join(entry.file_name());
            match destination.symlink_metadata() {
                Ok(_) => bail!(
                    "Swift restore would replace existing {}; use a fresh release source directory",
                    destination.display()
                ),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("checking Swift destination {}", destination.display())
                    })
                }
            }
        }
        let receipt = json!({"package_path": package, "scratch_path": scratch,
            "archive_sha256": digest, "reused": false});
        let mut moved = Vec::new();
        let placed = (|| -> Result<()> {
            for entry in entries {
                let destination = scratch.join(entry.file_name());
                fs::rename(entry.path(), &destination).with_context(|| {
                    format!("placing Swift cache entry {}", destination.display())
                })?;
                moved.push((entry.path(), destination));
            }
            atomic_json(&receipt_path, &receipt)
        })();
        if let Err(error) = placed {
            for (original, destination) in moved.into_iter().rev() {
                fs::rename(&destination, &original).with_context(|| {
                    format!(
                        "Swift restore failed: {error:#}; rollback of {} also failed",
                        destination.display()
                    )
                })?;
            }
            return Err(error);
        }
        Ok(receipt)
    })();
    let cleanup = fs::remove_dir_all(&stage);
    match (result, cleanup) {
        (Ok(receipt), Ok(())) => Ok(receipt),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(error).with_context(|| {
            format!(
                "Swift cache was restored but staging cleanup failed: {}",
                stage.display()
            )
        }),
        (Err(error), Err(cleanup)) => bail!(
            "Swift restore failed: {error:#}; staging cleanup also failed at {}: {cleanup}",
            stage.display()
        ),
    }
}
