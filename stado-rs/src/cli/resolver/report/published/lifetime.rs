//! Whether the resolver that published a state still runs, read from a lock
//! it holds for as long as it lives rather than from its pid.
//!
//! A pid outlives its process: once the resolver exits, the operating system
//! hands the number to any other process, and a reader asking whether that
//! pid runs is told yes about a resolver that is gone, so its last
//! `listening: true` sent traffic to ports nobody held. The resolver instead
//! holds an exclusive advisory lock on `resolver-state.lock` beside its
//! state from its first publication until it exits, and writes its pid into
//! that file while it holds it. The kernel drops the lock with the process,
//! however it ends. A reader that can take the lock knows no resolver runs;
//! one that finds it held reads whose it is, and trusts a published state
//! only when that holder wrote it.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The lock this process holds for its lifetime, once its first
/// publication took it: kept here so it is released only when the process
/// ends. A `OnceLock` because the path comes from the first publication.
static HELD: OnceLock<Result<File, String>> = OnceLock::new();

/// The lifetime lock beside `state`.
pub(super) fn lock_path(state: &Path) -> PathBuf {
    state.with_extension("lock")
}

/// Take the lifetime lock beside `state` for the rest of this process and
/// record this process as its holder; said on stderr when it cannot be
/// taken, and taken once however often the state is published.
pub(super) fn hold(state: &Path) {
    let held = HELD.get_or_init(|| take(&lock_path(state)));
    if let Err(cause) = held {
        eprintln!("stado resolver could not hold its lifetime lock: {cause}");
    }
}

fn take(path: &Path) -> Result<File, String> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|error| format!("opening {}: {error}", path.display()))?;
    match file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            return Err(format!(
                "{} is held by another running resolver: {}",
                path.display(),
                holder_of(&mut file)?
            ))
        }
        Err(TryLockError::Error(error)) => {
            return Err(format!("locking {}: {error}", path.display()))
        }
    }
    // The pid replaces whatever an earlier holder recorded: written from the
    // start, and the file cut where the pid ends.
    file.rewind()
        .and_then(|()| write!(file, "{}", std::process::id()))
        .and_then(|()| file.stream_position())
        .and_then(|end| file.set_len(end))
        .and_then(|()| file.sync_data())
        .map_err(|error| format!("recording this resolver in {}: {error}", path.display()))?;
    Ok(file)
}

/// The pid the holder recorded in the open lock file.
fn holder_of(file: &mut File) -> Result<String, String> {
    let mut recorded = String::new();
    file.rewind()
        .and_then(|()| file.read_to_string(&mut recorded))
        .map_err(|error| format!("reading the holder it recorded: {error}"))?;
    Ok(recorded.trim().to_string())
}

/// Whether the process holding the lifetime lock beside `state` is the one
/// whose pid is `publisher`: `Ok(false)` when no resolver holds it (no lock
/// file, or one this reader could take), and when a different resolver
/// holds it and has not published yet.
pub(super) fn held_by(state: &Path, publisher: u32) -> Result<bool, String> {
    let path = lock_path(state);
    let mut file = match File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("opening {}: {error}", path.display())),
    };
    match file.try_lock_shared() {
        // Taken by this reader: nobody holds it, and dropping the file
        // releases it again.
        Ok(()) => Ok(false),
        Err(TryLockError::WouldBlock) => {
            let holder =
                holder_of(&mut file).map_err(|cause| format!("{}: {cause}", path.display()))?;
            Ok(holder == publisher.to_string())
        }
        Err(TryLockError::Error(error)) => Err(format!("locking {}: {error}", path.display())),
    }
}
