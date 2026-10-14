//! Run markers, measurements and safe cleanup probes.

use super::{Run, COMMAND_RECORD, IN_USE, MEASURED};
use anyhow::{Context, Result};
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    time::SystemTime,
};
/// The size `run` took: the one it recorded when it was shed, else its own.
/// Only a missing record is measured afresh: a record that cannot be read,
/// measured afresh after the trees were shed, read as a few bytes and let a
/// build start that the volume could not hold.
pub(super) fn measured(run: &Path) -> Result<u64> {
    match fs::read_to_string(run.join(MEASURED)) {
        Ok(text) => text
            .trim()
            .parse()
            .with_context(|| format!("{} is not a byte count", run.join(MEASURED).display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => bytes(run),
        Err(error) => {
            Err(error).with_context(|| format!("reading {}", run.join(MEASURED).display()))
        }
    }
}

pub(super) fn create(parent: &Path, name: &str) -> Result<Run> {
    let path = parent.join(name);
    fs::create_dir_all(&path).with_context(|| format!("creating the run {}", path.display()))?;
    let in_use = File::create(path.join(IN_USE))
        .with_context(|| format!("marking the run {} in use", path.display()))?;
    match in_use.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => {
            anyhow::bail!("the new run {} is already in use", path.display());
        }
        Err(std::fs::TryLockError::Error(error)) => {
            return Err(error)
                .with_context(|| format!("marking the run {} in use", path.display()));
        }
    }
    Ok(Run {
        path,
        _in_use: in_use,
    })
}

/// The earlier runs under `parent`, newest first. A parent not created yet
/// holds none; one that cannot be listed, or a run whose time cannot be read,
/// is an error: dropped, the build skipped its free-space check or kept a run
/// it should have superseded, with nothing said. A run another process
/// removed between the listing and the read is no longer a run: every
/// command Stado runs writes its own record here, so the installation that
/// hands off to a second Stado lists the directory while the first prunes it.
pub(super) fn earlier(parent: &Path) -> Result<Vec<PathBuf>> {
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("listing the runs in {}", parent.display()))
        }
    };
    let mut runs: Vec<(SystemTime, PathBuf)> = Vec::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("listing the runs in {}", parent.display()))?;
        let path = entry.path();
        let kind = entry
            .file_type()
            .with_context(|| format!("reading the type of {}", path.display()))?;
        if !kind.is_dir() {
            continue;
        }
        let modified = match entry.metadata().and_then(|metadata| metadata.modified()) {
            Ok(modified) => modified,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("reading when {} was written", path.display()))
            }
        };
        runs.push((modified, path));
    }
    runs.sort_by_key(|run| std::cmp::Reverse(run.0));
    Ok(runs.into_iter().map(|(_, path)| path).collect())
}

/// Whether a command record says its command succeeded. An unreadable record
/// is kept: it is not evidence that nothing failed.
pub(super) fn succeeded(run: &Path) -> bool {
    fs::read(run.join(COMMAND_RECORD))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .is_some_and(|record| record["state"] == "succeeded")
}

/// Bytes of the regular files under `path`, symbolic links not followed. A
/// directory or file that cannot be read is an error naming it: counted as
/// nothing, the size recorded for the free-space check was too small.
pub(super) fn bytes(path: &Path) -> Result<u64> {
    let mut total = 0;
    for entry in fs::read_dir(path).with_context(|| format!("measuring {}", path.display()))? {
        let entry = entry.with_context(|| format!("measuring {}", path.display()))?;
        let entry_path = entry.path();
        let kind = entry
            .file_type()
            .with_context(|| format!("measuring {}", entry_path.display()))?;
        if kind.is_dir() {
            total += bytes(&entry_path)?;
        } else if kind.is_file() {
            total += entry
                .metadata()
                .with_context(|| format!("measuring {}", entry_path.display()))?
                .len();
        }
    }
    Ok(total)
}

pub(crate) fn gib(bytes: u64) -> f64 {
    bytes as f64 / f64::from(1u32 << 30)
}

/// Whether the run's creator still holds its marker. A run without a marker
/// was written by a Stado that did not mark runs, and is not in use. A marker
/// that exists and cannot be opened is counted as in use: read as free, a run
/// its creator was still writing was removed from under it.
pub(super) fn in_use(run: &Path) -> bool {
    match File::open(run.join(IN_USE)) {
        Ok(marker) => match marker.try_lock_shared() {
            Ok(()) => false,
            Err(std::fs::TryLockError::WouldBlock) => true,
            Err(std::fs::TryLockError::Error(error)) => {
                eprintln!(
                    "cannot inspect run lock {}: {error}; keeping the run",
                    run.display()
                );
                true
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => {
            eprintln!(
                "cannot open run lock {}: {error}; keeping the run",
                run.display()
            );
            true
        }
    }
}
