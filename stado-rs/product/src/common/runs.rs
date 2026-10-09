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
//!
//! An installation's parents live in the checkout's build area under Stado's
//! home ([`checkout_area`]), never in the checkout: the checkouts sit in
//! `~/Documents`, which macOS keeps from the janitor, so a tree written there
//! was one no pass could ever reclaim. A shed run is tagged with
//! `CACHEDIR.TAG`, so the janitor's build-cache cleaner takes it under disk
//! pressure; a run still being written is not tagged and is never taken.

use anyhow::{bail, Context, Result};
use fs2::FileExt;
use std::{
    fs::{self, File},
    path::{Component, Path, PathBuf},
    time::SystemTime,
};

/// Where Stado keeps what its builds of `checkout` write: the checkout's own
/// path below the home, under `~/.stado/products/builds`, so the area names
/// the checkout it belongs to and two checkouts never share one. A checkout
/// outside the home has no such path and is refused by name.
pub fn checkout_area(home: &Path, checkout: &Path) -> Result<PathBuf> {
    let below = checkout.strip_prefix(home).with_context(|| {
        format!(
            "{} is not below the home {}; Stado keeps a checkout's builds under \
             ~/.stado/products/builds/<the checkout's path below the home>",
            checkout.display(),
            home.display()
        )
    })?;
    let relative: PathBuf = below
        .components()
        .filter(|part| matches!(part, Component::Normal(_)))
        .collect();
    Ok(home.join(".stado/products/builds").join(relative))
}

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
/// every process on the host with it, not only itself. The previous attempt
/// keeps only its evidence: a run a Stado that did not shed left whole loses
/// its trees here, its size recorded first.
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
        if !in_use(&newest) {
            shed(&newest)?;
        }
        let needed = measured(&newest)?;
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

/// The file a shed run keeps the size it took before its trees were removed.
const MEASURED: &str = "measured-bytes";

/// Remove every directory of a finished `run` — the trees a build writes and
/// nothing reads after it ends — and keep its files, the run's evidence, with
/// the size the whole run took, which the next build's free-space check reads.
/// A run already shed keeps the size it recorded then.
pub fn shed(run: &Path) -> Result<()> {
    if !run.join(MEASURED).is_file() {
        let size = bytes(run);
        fs::write(run.join(MEASURED), size.to_string())
            .with_context(|| format!("recording the size of {}", run.display()))?;
    }
    for entry in fs::read_dir(run).with_context(|| format!("reading {}", run.display()))? {
        let entry = entry.with_context(|| format!("reading {}", run.display()))?;
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            fs::remove_dir_all(entry.path())
                .with_context(|| format!("removing {}", entry.path().display()))?;
        }
    }
    super::tag_cache(run, "stado, once the run ended")
}

/// The size `run` took: the one it recorded when it was shed, else its own.
fn measured(run: &Path) -> Result<u64> {
    match fs::read_to_string(run.join(MEASURED)) {
        Ok(text) => text
            .trim()
            .parse()
            .with_context(|| format!("{} is not a byte count", run.join(MEASURED).display())),
        Err(_) => Ok(bytes(run)),
    }
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

pub(crate) fn gib(bytes: u64) -> f64 {
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
