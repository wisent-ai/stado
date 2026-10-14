use anyhow::{bail, Context, Result};
use fs2::FileExt;
use serde_json::Value;
use sha2::{Digest, Sha256};
use stado_wait as wait;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::Path,
};

/// The first line of a `CACHEDIR.TAG` that declares its directory
/// regenerable, by the Cache Directory Tagging Specification
/// (https://bford.info/cachedir/).
pub const CACHEDIR_SIGNATURE: &str = "Signature: 8a477f597d28d172789f06886806bc55";

/// Mark `directory` as a tree its build tool regenerates, so the janitor's
/// build-cache cleaner may reclaim it whole under disk pressure. `writer`
/// names who wrote the tag, for a reader of the file.
pub fn tag_cache(directory: &Path, writer: &str) -> Result<()> {
    fs::create_dir_all(directory).with_context(|| format!("creating {}", directory.display()))?;
    fs::write(
        directory.join("CACHEDIR.TAG"),
        format!(
            "{CACHEDIR_SIGNATURE}\n# Written by {writer}: the next build reproduces this tree.\n"
        ),
    )
    .with_context(|| format!("tagging {}", directory.display()))
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("output has no parent directory")?;
    fs::create_dir_all(parent)?;
    if path
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        bail!("refusing symlinked output {}", path.display());
    }
    let temporary = parent.join(format!(
        ".{}-{}.tmp",
        path.file_name()
            .context("missing filename")?
            .to_string_lossy(),
        uuid::Uuid::new_v4()
    ));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        if let Ok(metadata) = path.metadata() {
            file.set_permissions(metadata.permissions())?;
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub fn atomic_json(path: &Path, value: &Value) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    atomic_write(path, &bytes)
}

pub fn sha256(path: &Path) -> Result<String> {
    let mut file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let length = file.read(&mut buffer)?;
        if length == 0 {
            break;
        }
        hash.update(&buffer[..length]);
    }
    Ok(hex::encode(hash.finalize()))
}

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
    take(path, Take::Refuse)
}

/// Take the exclusive lock at `path`, blocking until the current holder
/// releases it. For an operator who asked (`--wait`) to run after a
/// concurrent install of the same surface rather than replace it.
pub fn lock_waiting(path: &Path) -> Result<File> {
    take(path, Take::Wait)
}

/// Take the exclusive lock at `path`, replacing a holder still preparing
/// ([`Take::Supersede`]).
pub fn lock_superseding(path: &Path) -> Result<File> {
    take(path, Take::Supersede)
}

fn take(path: &Path, mode: Take) -> Result<File> {
    fs::create_dir_all(path.parent().context("lock has no parent")?)?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(path)?;
    if let Err(error) = file.try_lock_exclusive() {
        let holder = crate::install::operation::clause(path);
        let held_by = match mode {
            Take::Refuse => {
                // The io::Error stays the source, so a caller that treats
                // WouldBlock as "busy" (the reconciliation sweep) still recognizes it.
                return Err(anyhow::Error::new(error)
                    .context(format!("another writer owns {}; {holder}", path.display())));
            }
            Take::Wait => holder,
            Take::Supersede => match record(path) {
                // The same installation, asked again: stopping the holder
                // only restarts the work it has done. Two sessions installing
                // Stado on one laptop each stopped the other's build as soon
                // as it started, and neither ever placed a file while the
                // host's own Stado crash-looped on the old one.
                Some(_) if recorded_command(path).is_some_and(|command| same_request(&command)) => {
                    format!("{holder} (the same installation, already under way)")
                }
                // A holder still preparing has placed nothing, and one past
                // placement has nothing left that changes the surface: either
                // is stopped for the newer installation. Only placing waits.
                Some((pid, phase))
                    if (phase == PHASE_PREPARING || phase == PHASE_PLACED) && alive(pid) =>
                {
                    eprintln!(
                        "superseding the installation {holder}: it is {phase}, not placing files"
                    );
                    // SAFETY: a signal to a pid read from the lock record this
                    // process could not take; a pid that is gone answers ESRCH.
                    match unsafe { libc::kill(pid, libc::SIGTERM) }.is_negative() {
                        false => format!("{holder} (told to stop)"),
                        true => format!(
                            "{holder} (pid {pid} could not be told to stop: {})",
                            std::io::Error::last_os_error()
                        ),
                    }
                }
                _ => format!("{holder} (it is placing files)"),
            },
        };
        wait::blocking(
            wait::Kind::Lock,
            format!("the lock held by {held_by}"),
            path.display(),
            || file.lock_exclusive(),
        )
        .with_context(|| format!("waiting for {} failed", path.display()))?;
    }
    let taken = chrono::Utc::now().to_rfc3339();
    write_record(&mut file, PHASE_PREPARING, None, &taken)?;
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
    let held: Value = serde_json::from_str(fs::read_to_string(path)?.trim())
        .with_context(|| format!("{} holds no readable holder record", path.display()))?;
    let recorded = |name: &str| {
        held[name]
            .as_str()
            .map(str::to_owned)
            .with_context(|| format!("{} records no {name}", path.display()))
    };
    let phase = match phase {
        Some(phase) => phase.to_owned(),
        None => recorded("phase")?,
    };
    let mut file = OpenOptions::new().write(true).open(path)?;
    write_record(&mut file, &phase, step, &recorded("acquired_at")?)
}

fn write_record(file: &mut File, phase: &str, step: Option<&str>, taken: &str) -> Result<()> {
    let record = serde_json::json!({
        "pid": std::process::id(),
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

/// The pid and phase a lock file records; `None` when it records nothing
/// readable. A record without a phase is from a Stado that recorded none,
/// and is treated as placing: never stopped, only waited for.
fn record(path: &Path) -> Option<(i32, String)> {
    let text = fs::read_to_string(path).ok()?;
    let record: Value = serde_json::from_str(text.trim()).ok()?;
    let pid = i32::try_from(record["pid"].as_u64()?).ok()?;
    let phase = record["phase"].as_str().unwrap_or(PHASE_PLACING).to_owned();
    Some((pid, phase))
}

/// The command line a lock file records, when it records one.
fn recorded_command(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let record: Value = serde_json::from_str(text.trim()).ok()?;
    Some(record["command"].as_str()?.to_owned())
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
