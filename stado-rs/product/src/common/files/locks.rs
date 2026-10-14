use super::record;
use anyhow::{Context, Result};
use stado_wait as wait;
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::Path,
};

/// How a lock is taken when another process holds it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Take {
    /// Refuse, naming the holder: what a reconciliation sweep does, because a
    /// busy surface is not its problem.
    Refuse,
    /// Block until the holder releases it, whatever it is doing.
    Wait,
    /// A newer installation of the same surface replaces an older one that
    /// has not started placing files, or has placed and recorded them (what
    /// it still does does not change the surface): the holder is told to stop
    /// (SIGTERM) and the lock is taken once it has gone. A holder placing is
    /// waited for, because placement is short and a half-placed surface is
    /// worse than a late one. This is what `install` and `update` do, as a
    /// newer fleet build cancels the older builds it supersedes.
    Supersede,
}

/// The phase a lock holder records: `preparing` (building, nothing placed),
/// `placing` (files being replaced) or `placed` (its files are in place and
/// recorded; what remains does not change the surface).
pub const PHASE_PREPARING: &str = "preparing";
pub const PHASE_PLACING: &str = "placing";
pub const PHASE_PLACED: &str = "placed";

/// Take the exclusive lock at `path` and write who holds it into the file.
///
/// A refusal names that holder — its pid, whether the pid is still alive, its
/// command line and since when — because "another writer owns" alone left two
/// sessions installing Stado unable to tell a running install from a stuck one.
pub fn lock(path: &Path) -> Result<File> {
    take(path, Take::Refuse, false)
}

/// Wait for a generic resource lock, without treating its parent directory
/// as a product identity.
pub fn lock_waiting(path: &Path) -> Result<File> {
    take(path, Take::Wait, false)
}

/// Wait for one product surface's lock and validate the product it guards.
pub fn lock_surface_waiting(path: &Path) -> Result<File> {
    take(path, Take::Wait, true)
}

/// A newer surface installation supersedes a verified holder before placement.
pub fn lock_superseding(path: &Path) -> Result<File> {
    take(path, Take::Supersede, true)
}

fn take(path: &Path, mode: Take, product_scope: bool) -> Result<File> {
    let expected = if product_scope {
        Some(record::product(path)?)
    } else {
        None
    };
    fs::create_dir_all(path.parent().context("lock has no parent")?)?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(path)?;
    if let Err(error) = file.try_lock() {
        match error {
            std::fs::TryLockError::WouldBlock => {}
            std::fs::TryLockError::Error(error) => {
                return Err(error).with_context(|| format!("taking {}", path.display()));
            }
        }
        let recorded = record::read(path, expected)?
            .with_context(|| format!("{} is held but records no holder", path.display()))?;
        if let Some(expected) = expected {
            recorded.validate_product(path, expected)?;
        }
        let holder = crate::install::operation::clause(path);
        let held_by = match mode {
            Take::Refuse => {
                anyhow::bail!("another writer owns {}; {holder}", path.display());
            }
            Take::Wait => holder,
            Take::Supersede if same_request(&recorded.command) => {
                format!("{holder} (the same installation, already under way)")
            }
            Take::Supersede
                if (recorded.phase == PHASE_PREPARING || recorded.phase == PHASE_PLACED)
                    && recorded.verified_live()? =>
            {
                let pid = recorded.pid;
                eprintln!(
                    "superseding the installation {holder}: it is {}, not placing files",
                    recorded.phase
                );
                // SAFETY: the process table still names the recorded PID and birth time.
                match unsafe { libc::kill(pid, libc::SIGTERM) }.is_negative() {
                    false => format!("{holder} (told to stop)"),
                    true => format!(
                        "{holder} (pid {pid} could not be told to stop: {})",
                        std::io::Error::last_os_error()
                    ),
                }
            }
            Take::Supersede => {
                format!("{holder} (it is placing files or its process identity is unverified)")
            }
        };
        wait::blocking_on_lock(format!("the lock held by {held_by}"), path, || file.lock())
            .with_context(|| format!("waiting for {} was refused", path.display()))?
            .with_context(|| format!("waiting for {} failed", path.display()))?;
    }
    if let Some(previous) = record::read(path, expected)? {
        let observed = previous.observed_start()?;
        eprintln!(
            "reclaiming released lock {}: guarded product {:?}, recorded product {:?}, pid {} born {:?}, observed {:?}, command {:?}, phase {}, step {:?}; no process was signalled",
            path.display(), expected, previous.product, previous.pid,
            previous.started_at, observed, previous.command, previous.phase, previous.step
        );
    }
    let taken = chrono::Utc::now().to_rfc3339();
    let pid = i32::try_from(std::process::id())?;
    let started = super::super::process::started_at(pid)?.with_context(|| {
        format!(
            "installing process pid {pid} has exited before locking {}",
            path.display()
        )
    })?;
    write_record(
        &mut file,
        path,
        PHASE_PREPARING,
        None,
        &taken,
        &started,
        product_scope,
    )?;
    Ok(file)
}

/// Record that the holder of `path` (this process) has started placing
/// files, so a newer installation waits for it instead of stopping it.
pub fn mark_placing(path: &Path) -> Result<()> {
    remark(path, Some(PHASE_PLACING), None)
}

/// Record that the holder of `path` (this process) has placed and recorded
/// its files, so a newer installation supersedes it instead of waiting.
pub fn mark_placed(path: &Path) -> Result<()> {
    remark(path, Some(PHASE_PLACED), None)
}

/// Record the step the holder of `path` (this process) is in, keeping its
/// phase, so a waiter and `stado product status` name what the lock waits on.
pub fn mark_step(path: &Path, step: &str) -> Result<()> {
    remark(path, None, Some(step))
}

/// Rewrite this process's own record of `path`, keeping when it took it and,
/// unless `phase` names a new one, its phase.
fn remark(path: &Path, phase: Option<&str>, step: Option<&str>) -> Result<()> {
    let held = record::read(path, Some(record::product(path)?))?
        .with_context(|| format!("{} holds no readable holder record", path.display()))?;
    held.validate_product(path, record::product(path)?)?;
    if held.pid != i32::try_from(std::process::id())? || !held.verified_live()? {
        anyhow::bail!(
            "{} records pid {} with an unverified birth time, not this process; its step is not rewritten",
            path.display(), held.pid
        );
    }
    let phase = match phase {
        Some(phase) => phase,
        None => &held.phase,
    };
    let started = held
        .started_at
        .as_deref()
        .with_context(|| format!("{} records no process birth time", path.display()))?;
    let mut options = OpenOptions::new();
    options.write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(path)?;
    write_record(
        &mut file,
        path,
        phase,
        step,
        &held.acquired_at,
        started,
        true,
    )
}

fn write_record(
    file: &mut File,
    path: &Path,
    phase: &str,
    step: Option<&str>,
    taken: &str,
    started: &str,
    product_scope: bool,
) -> Result<()> {
    let product = if product_scope {
        Some(record::product(path)?)
    } else {
        None
    };
    let record = serde_json::json!({
        "product": product,
        "pid": std::process::id(),
        "started_at": started,
        "command": std::env::args().collect::<Vec<_>>().join(" "),
        "acquired_at": taken,
        "phase": phase,
        "step": step,
    });
    file.set_len(0)?;
    file.write_all(format!("{record}\n").as_bytes())?;
    file.flush()?;
    Ok(())
}

/// Whether a recorded command asks for what this process asks for: the same
/// arguments after the program's own path, whatever output format each
/// chose, since `--json` changes what is printed, not what is installed.
fn same_request(recorded: &str) -> bool {
    let request = |words: Vec<String>| -> Vec<String> {
        words
            .split_first()
            .map(|(_, arguments)| arguments.to_vec())
            .into_iter()
            .flatten()
            .filter(|word| word != "--json")
            .collect()
    };
    request(recorded.split_whitespace().map(str::to_owned).collect())
        == request(std::env::args().collect())
}

pub fn alive(pid: i32) -> bool {
    // Signal 0 delivers nothing and only asks whether the pid exists.
    unsafe { libc::kill(pid, 0) == 0 }
}
