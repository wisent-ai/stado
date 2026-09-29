//! Per-run directories a Stado command writes beneath a checkout, and how
//! many of them one parent keeps.
//!
//! A source install, a native build, a cargo install's staging and a recorded
//! command each write a fresh `<parent>/<run>`, and none of them removed the
//! runs before it, so a checkout grew by a build's worth of disk on every
//! attempt. Every such parent now keeps its newest few runs; a run whose
//! creator is still working is never removed, whatever its age.

use anyhow::{bail, Context, Result};
use fs2::FileExt;
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    time::SystemTime,
};

/// Earlier builds a parent keeps: a source install's export, a native build's
/// sources, a cargo install's target directory. Each can be over a gigabyte.
pub const KEPT_BUILDS: usize = 3;
/// Earlier recorded commands a parent keeps. A record is its logs, and an
/// error names its directory for the reader who comes to look.
pub const KEPT_COMMAND_RECORDS: usize = 200;

/// The file a run's creator holds exclusively while the run is in use.
const IN_USE: &str = ".in-use";

/// A fresh run directory, marked in use for as long as this value lives.
pub struct Run {
    pub path: PathBuf,
    _in_use: File,
}

/// Create `<parent>/<name>` after removing every earlier run but the newest
/// `kept`. A run whose creator still holds it is skipped, not removed.
pub fn fresh(parent: &Path, name: &str, kept: usize) -> Result<Run> {
    prune(parent, kept)?;
    create(parent, name)
}

/// A build run: [`fresh`] with [`KEPT_BUILDS`], refused before anything is
/// written when the volume holds less free space than the newest earlier run
/// of this parent took. A build that ran out of disk would fail every process
/// on the host with it, not only itself.
pub fn fresh_build(parent: &Path, name: &str) -> Result<Run> {
    if let Some(newest) = prune(parent, KEPT_BUILDS)? {
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

/// Remove all but the newest `kept` runs; answer the newest run left.
fn prune(parent: &Path, kept: usize) -> Result<Option<PathBuf>> {
    let Ok(entries) = fs::read_dir(parent) else {
        return Ok(None);
    };
    let mut earlier: Vec<(SystemTime, PathBuf)> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .collect();
    earlier.sort_by(|left, right| right.0.cmp(&left.0));
    let newest = earlier.first().map(|(_, path)| path.clone());
    for (_, stale) in earlier.into_iter().skip(kept) {
        if in_use(&stale) {
            continue;
        }
        fs::remove_dir_all(&stale)
            .with_context(|| format!("removing the earlier run {}", stale.display()))?;
    }
    Ok(newest)
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
