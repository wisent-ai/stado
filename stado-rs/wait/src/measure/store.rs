//! The longest completed wait on each operation, kept on this machine.
//!
//! The directory is `$STADO_WAITS_MEASURED_DIR` when set, else
//! `$HOME/.stado/waits-measured`. Each operation is one file named by the
//! SHA-256 of the operation in hex, a fixed length whatever the operation
//! holds, and holding the longest completed wait in whole nanoseconds, then a
//! space and the operation, so the directory can be listed by operation. The
//! directory holds one file per operation that ever completed a wait here.
//!
//! A completion takes an exclusive lock on the operation's lock file (the
//! record's name with `.lock`, which is never replaced), reads the kept value
//! again under it, and replaces the record (written beside it, then renamed,
//! so a reader never sees half a record) only when the completion is longer:
//! two waits on one operation that complete together, in two processes or two
//! threads, keep the longer whatever order they end.

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

/// The variable that names the measurement directory in place of the home
/// one.
pub const DIRECTORY_ENV: &str = "STADO_WAITS_MEASURED_DIR";

/// The measurement directory, or why this process has none.
pub fn directory() -> Result<PathBuf, String> {
    if let Some(explicit) = std::env::var_os(DIRECTORY_ENV).filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(explicit));
    }
    match std::env::var_os("HOME").filter(|value| !value.is_empty()) {
        Some(home) => Ok(PathBuf::from(home).join(".stado").join("waits-measured")),
        None => Err(format!(
            "neither {DIRECTORY_ENV} nor HOME is set, so the measured waits have no directory"
        )),
    }
}

/// The file `operation` is kept in.
fn file(directory: &Path, operation: &str) -> PathBuf {
    let digest = Sha256::digest(operation.as_bytes());
    directory.join(format!("{digest:x}"))
}

/// The duration a kept record holds, or why it holds none.
fn parse(file: &Path, text: &str) -> Result<Duration, String> {
    let nanos = match text.split_once(' ') {
        Some((nanos, _)) => nanos,
        None => text.trim(),
    };
    let nanos: u64 = nanos.parse().map_err(|error| {
        format!(
            "{} holds {nanos:?}, not nanoseconds: {error}",
            file.display()
        )
    })?;
    Ok(Duration::from_nanos(nanos))
}

/// The longest completed wait on `operation`, none when no wait on it has
/// completed here, or why the measurement cannot be read.
pub(crate) fn longest(operation: &str) -> Result<Option<Duration>, String> {
    let file = file(&directory()?, operation);
    match std::fs::read_to_string(&file) {
        Ok(text) => parse(&file, &text).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("reading {}: {error}", file.display())),
    }
}

/// Keep `took` as the longest completed wait on `operation` when it is
/// longer than the one kept.
pub(crate) fn record(operation: &str, took: Duration) -> Result<(), String> {
    if longest(operation)?.is_some_and(|kept| kept >= took) {
        return Ok(());
    }
    let directory = directory()?;
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("creating {}: {error}", directory.display()))?;
    let file = file(&directory, operation);
    let lock = file.with_extension("lock");
    let held = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock)
        .map_err(|error| format!("opening {}: {error}", lock.display()))?;
    held.lock()
        .map_err(|error| format!("locking {}: {error}", lock.display()))?;
    if longest(operation)?.is_some_and(|kept| kept >= took) {
        return Ok(());
    }
    let beside = file.with_extension(format!(
        "{}.{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::write(&beside, format!("{} {operation}\n", took.as_nanos()))
        .map_err(|error| format!("writing {}: {error}", beside.display()))?;
    std::fs::rename(&beside, &file).map_err(|error| {
        format!(
            "renaming {} to {}: {error}",
            beside.display(),
            file.display()
        )
    })
}
