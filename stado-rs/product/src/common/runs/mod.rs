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
//!   command's record is what its error names, so it stays for the reader
//!   until a newer record of the same command (program, arguments,
//!   directory) supersedes it.
//!
//! Command records are pruned once per process, by its first recorded
//! command: a process that runs many commands reads the earlier records once,
//! not once for each command it runs.
//!
//! An installation's parents live in the checkout's build area under Stado's
//! home ([`checkout_area`]), never in the checkout: the checkouts sit in
//! `~/Documents`, which macOS keeps from the janitor, so a tree written there
//! was one no pass could ever reclaim. A shed run is tagged with
//! `CACHEDIR.TAG`, so the janitor's build-cache cleaner takes it under disk
//! pressure; a run still being written is not tagged and is never taken.

mod abandoned;
mod probes;
pub use abandoned::reclaim_abandoned;
pub(crate) use probes::gib;
use probes::{bytes, create, earlier, in_use, measured, succeeded};

use anyhow::{bail, Context, Result};
use std::{
    collections::BTreeSet,
    fs::{self, File},
    path::{Component, Path, PathBuf},
    sync::{LazyLock, Mutex},
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
    let (area, path) = if checkout.starts_with(home) {
        (builds, checkout.strip_prefix(home)?)
    } else {
        (builds.join(OUTSIDE_HOME), checkout)
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

/// The record parents this process has pruned.
static PRUNED: LazyLock<Mutex<BTreeSet<PathBuf>>> = LazyLock::new(Mutex::default);

/// A command record: create `<parent>/<name>`, after removing, the first time
/// this process writes under `parent`, every earlier record a later one
/// supersedes. A record still held is skipped.
pub fn fresh_record(parent: &Path, name: &str) -> Result<Run> {
    let mut pruned = PRUNED
        .lock()
        .map_err(|poisoned| anyhow::anyhow!("the pruned record parents: {poisoned}"))?;
    if !pruned.contains(parent) {
        prune_records(parent)?;
        pruned.insert(parent.to_path_buf());
    }
    drop(pruned);
    create(parent, name)
}

/// Remove every record under `parent` whose command succeeded, and every
/// failed command's record but the newest of that same command. A prune that
/// removed records says on stderr how many it removed and how many it kept.
fn prune_records(parent: &Path) -> Result<()> {
    let earlier = earlier(parent)?;
    let mut newest = BTreeSet::new();
    let mut removed = Vec::new();
    for run in &earlier {
        if in_use(run) {
            continue;
        }
        if succeeded(run) {
            remove_superseded(run, "record")?;
            removed.push(run);
            continue;
        }
        let Some(command) = command_of(run) else {
            continue;
        };
        if !newest.insert(command) {
            remove_superseded(run, "record")?;
            removed.push(run);
        }
    }
    if !removed.is_empty() {
        eprintln!(
            "stado: removed {} superseded command records under {}; {} kept",
            removed.len(),
            parent.display(),
            earlier.len() - removed.len()
        );
    }
    Ok(())
}

/// The command a record ran — its program, arguments and directory as the
/// record states them. A record that cannot be read names no command and is
/// kept, said on stderr: it is not evidence of which command it was.
fn command_of(run: &Path) -> Option<String> {
    let record = run.join(COMMAND_RECORD);
    let bytes = match fs::read(&record) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            eprintln!(
                "cannot read {}: {error}; keeping the record",
                record.display()
            );
            return None;
        }
    };
    match serde_json::from_slice::<serde_json::Value>(&bytes) {
        Ok(stated) => {
            Some(serde_json::json!([stated["program"], stated["args"], stated["cwd"]]).to_string())
        }
        Err(error) => {
            eprintln!(
                "cannot parse {}: {error}; keeping the record",
                record.display()
            );
            None
        }
    }
}

/// Remove a superseded run. A run another process removed first, wholly or
/// part way through, is the state this removal was for: every Stado that
/// writes under one parent prunes it, so two of them reach the same run.
fn remove_superseded(run: &Path, what: &str) -> Result<()> {
    match fs::remove_dir_all(run) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            Err(error).with_context(|| format!("removing the earlier {what} {}", run.display()))
        }
        _ => Ok(()),
    }
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
        remove_superseded(&stale, "run")?;
    }
    if let Some(newest) = previous {
        if !in_use(&newest) && !kept(&newest) {
            shed(&newest)?;
        }
        let needed = measured(&newest)?;
        let free = super::files::available_space(parent)?;
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
        path.parent()
            .with_context(|| format!("the run {} has no parent", path.display()))?,
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
/// trees are reproduced by a build, so `stado space reclaim` and the
/// janitor's build-cache cleaner may take it under disk pressure.
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
            // Two builds under one parent shed the same previous run; a
            // tree the other removed first is the state this removal was for.
            remove_superseded(&entry.path(), "tree")?;
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
