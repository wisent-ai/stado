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

/// The directory under `~/.stado/products/builds` holding the build areas of
/// checkouts that are not below the home, by their whole absolute path.
const OUTSIDE_HOME: &str = "outside-home";

/// Where Stado keeps what its builds of `checkout` write, under
/// `~/.stado/products/builds`: the checkout's own path below the home, or
/// `outside-home/<its absolute path>` for a checkout elsewhere, so the area
/// names the checkout it belongs to and two checkouts never share one.
pub fn checkout_area(home: &Path, checkout: &Path) -> Result<PathBuf> {
    if !checkout.is_absolute() {
        bail!(
            "{} is not an absolute path; Stado names a checkout's build area from its absolute \
             path",
            checkout.display()
        );
    }
    let builds = home.join(".stado/products/builds");
    let (area, path) = match checkout.strip_prefix(home) {
        Ok(below) => (builds, below),
        Err(_) => (builds.join(OUTSIDE_HOME), checkout),
    };
    Ok(area.join(
        path.components()
            .filter(|part| matches!(part, Component::Normal(_)))
            .collect::<PathBuf>(),
    ))
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
    for run in earlier(parent)? {
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
    let mut runs = earlier(parent)?.into_iter();
    let previous = runs.next();
    for stale in runs {
        if in_use(&stale) || kept(&stale) {
            continue;
        }
        fs::remove_dir_all(&stale)
            .with_context(|| format!("removing the earlier run {}", stale.display()))?;
    }
    if let Some(newest) = previous {
        if !in_use(&newest) && !kept(&newest) {
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

/// Hold the existing run at `path` in use for as long as the returned value
/// lives: an install resuming a build an earlier install kept.
pub fn hold(path: &Path) -> Result<Run> {
    create(
        path.parent().with_context(|| format!("the run {} has no parent", path.display()))?,
        path.file_name()
            .and_then(|name| name.to_str())
            .with_context(|| format!("the run {} has no name", path.display()))?,
    )
}

/// The file a run's creator writes to keep the run's directories past its
/// end: a build that passed and whose install did not complete, which the
/// next install of the same revision resumes instead of building again.
const KEPT: &str = ".kept";

/// Keep `run` whole past its end, for `why`, and tag it as a cache: its
/// trees are reproduced by a build, so `stado product build-trees remove`
/// and the janitor under disk pressure may take it.
pub fn keep(run: &Path, why: &str) -> Result<()> {
    fs::write(run.join(KEPT), format!("{why}\n"))
        .with_context(|| format!("keeping the run {}", run.display()))?;
    super::tag_cache(run, "stado, keeping a build that passed")
}

/// Whether `run` is kept past its end. A marker that cannot be read is
/// counted as kept: read as free, a build the next install resumes was shed.
pub fn kept(run: &Path) -> bool {
    match fs::metadata(run.join(KEPT)) {
        Ok(_) => true,
        Err(error) => error.kind() != std::io::ErrorKind::NotFound,
    }
}

/// The file a shed run keeps the size it took before its trees were removed.
const MEASURED: &str = "measured-bytes";

/// Remove every directory of a finished `run` — the trees a build writes and
/// nothing reads after it ends — and keep its files, the run's evidence, with
/// the size the whole run took, which the next build's free-space check reads.
/// A run already shed keeps the size it recorded then. A run kept past its
/// end ([`keep`]) is kept no longer: its install completed.
pub fn shed(run: &Path) -> Result<()> {
    if !run.join(MEASURED).is_file() {
        let size = bytes(run)?;
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
    match fs::remove_file(run.join(KEPT)) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("releasing the kept run {}", run.display()))
        }
    }
    super::tag_cache(run, "stado, once the run ended")
}

/// The size `run` took: the one it recorded when it was shed, else its own.
/// Only a missing record is measured afresh: a record that cannot be read,
/// measured afresh after the trees were shed, read as a few bytes and let a
/// build start that the volume could not hold.
fn measured(run: &Path) -> Result<u64> {
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

/// The earlier runs under `parent`, newest first. A parent not created yet
/// holds none; one that cannot be listed, or a run whose time cannot be read,
/// is an error: dropped, the build skipped its free-space check or kept a run
/// it should have superseded, with nothing said.
fn earlier(parent: &Path) -> Result<Vec<PathBuf>> {
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
        let modified = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .with_context(|| format!("reading when {} was written", path.display()))?;
        runs.push((modified, path));
    }
    runs.sort_by_key(|run| std::cmp::Reverse(run.0));
    Ok(runs.into_iter().map(|(_, path)| path).collect())
}

/// Whether a command record says its command succeeded. An unreadable record
/// is kept: it is not evidence that nothing failed.
fn succeeded(run: &Path) -> bool {
    fs::read(run.join(COMMAND_RECORD))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .is_some_and(|record| record["state"] == "succeeded")
}

/// Bytes of the regular files under `path`, symbolic links not followed. A
/// directory or file that cannot be read is an error naming it: counted as
/// nothing, the size recorded for the free-space check was too small.
fn bytes(path: &Path) -> Result<u64> {
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
fn in_use(run: &Path) -> bool {
    match File::open(run.join(IN_USE)) {
        Ok(marker) => marker.try_lock_shared().is_err(),
        Err(error) => error.kind() != std::io::ErrorKind::NotFound,
    }
}
