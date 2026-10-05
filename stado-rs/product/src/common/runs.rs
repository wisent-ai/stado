//! Per-run directories a Stado command writes beneath a checkout, and which
//! of them one parent keeps.
//!
//! A source install, a native build, a cargo install's staging and a recorded
//! command each write a fresh `<parent>/<run>`, and none of them removed the
//! runs before it, so a checkout grew by a build's worth of disk on every
//! attempt. A run is now removed once a later run supersedes it, and a run
//! whose creator is still working is never removed, whatever its age:
//!
//! - a build keeps the previous attempt, the evidence of the last outcome and
//!   the measure of what the next one needs; every older build is superseded
//!   by it;
//! - a command record is superseded once its command succeeded; a failed
//!   command's record is what its error names, so it stays for the reader.

use anyhow::{bail, Context, Result};
use fs2::FileExt;
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    time::SystemTime,
};

/// The record a command run writes; its `state` says how the command ended.
pub const COMMAND_RECORD: &str = "command.json";

/// The file a run's creator holds exclusively while the run is in use.
const IN_USE: &str = ".in-use";
/// A fresh run directory, marked in use for as long as this value lives.
pub struct Run {
    pub path: PathBuf,
    _in_use: File,
}

/// A command record: create `<parent>/<name>` after removing every earlier
/// record whose command succeeded. A record still held is skipped.
pub fn fresh_record(parent: &Path, name: &str) -> Result<Run> {
    for run in earlier(parent) {
        if !in_use(&run) && succeeded(&run) {
            fs::remove_dir_all(&run)
                .with_context(|| format!("removing the earlier record {}", run.display()))?;
        }
    }
    create(parent, name)
}

/// A build run, after removing every earlier build but the previous attempt,
/// refused before anything is written when the volume holds less free space
/// than that previous attempt took. A build that ran out of disk would fail
/// every process on the host with it, not only itself.
pub fn fresh_build(parent: &Path, name: &str) -> Result<Run> {
    let mut runs = earlier(parent).into_iter();
    let previous = runs.next();
    for stale in runs {
        if in_use(&stale) {
            continue;
        }
        fs::remove_dir_all(&stale)
            .with_context(|| format!("removing the earlier run {}", stale.display()))?;
    }
    if let Some(newest) = previous {
        let needed = bytes(&newest);
        let free = fs2::available_space(parent)
            .with_context(|| format!("reading the free space under {}", parent.display()))?;
        if free < needed {
            bail!(
                "{:.1} GiB are free under {} and the previous run there, {}, took {:.1} GiB; \
                 a build now would fill the volume. Free space on this host first (`stado space \
                 reclaim <host>`), then run it again",
                gib(free),
                parent.display(),
                newest.display(),
                gib(needed)
            );
        }
    }
    create(parent, name)
}

fn create(parent: &Path, name: &str) -> Result<Run> {
    let path = parent.join(name);
    fs::create_dir_all(&path).with_context(|| format!("creating the run {}", path.display()))?;
    let in_use = File::create(path.join(IN_USE))
        .with_context(|| format!("marking the run {} in use", path.display()))?;
    in_use
        .try_lock_exclusive()
        .with_context(|| format!("marking the run {} in use", path.display()))?;
    Ok(Run {
        path,
        _in_use: in_use,
    })
}

/// The earlier runs under `parent`, newest first.
fn earlier(parent: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut runs: Vec<(SystemTime, PathBuf)> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .collect();
    runs.sort_by_key(|run| std::cmp::Reverse(run.0));
    runs.into_iter().map(|(_, path)| path).collect()
}

/// Whether a command record says its command succeeded. An unreadable record
/// is kept: it is not evidence that nothing failed.
fn succeeded(run: &Path) -> bool {
    fs::read(run.join(COMMAND_RECORD))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .is_some_and(|record| record["state"] == "succeeded")
}

/// Bytes of the regular files under `path`, symbolic links not followed.
fn bytes(path: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    entries
        .filter_map(|entry| entry.ok())
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => bytes(&entry.path()),
            Ok(kind) if kind.is_file() => entry.metadata().map_or(0, |data| data.len()),
            _ => 0,
        })
        .sum()
}

fn gib(bytes: u64) -> f64 {
    bytes as f64 / f64::from(1u32 << 30)
}

/// Whether the run's creator still holds its marker. A run without a marker
/// was written by a Stado that did not mark runs, and is not in use.
fn in_use(run: &Path) -> bool {
    let Ok(marker) = File::open(run.join(IN_USE)) else {
        return false;
    };
    marker.try_lock_shared().is_err()
}
