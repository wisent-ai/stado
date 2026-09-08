//! The lock file itself, its holder record, and the exclusive hold.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::providers::local::disk_cleanup::janitor::pass::lock::euid;
use crate::providers::local::disk_cleanup::janitor::state::error::JanitorError;
use crate::providers::local::disk_cleanup::janitor::state::report::build::epoch_now;
use crate::providers::local::disk_cleanup::janitor::{
    LOCK_HOLDER_INODE_PREFIX, LOCK_HOLDER_NAME, LOCK_NAME,
};

/// Python `_open_lock`: open `disk-cleanup.lock` with O_RDWR|O_CREAT|
/// O_NOFOLLOW, verify it is a regular file owned by us, force 0600.
pub(crate) fn open_lock(state_dir: &Path) -> Result<File, JanitorError> {
    open_lock_at(&state_dir.join(LOCK_NAME))
}

/// The same checks at an exact path, for the takeover's staged file.
pub(crate) fn open_lock_at(path: &Path) -> Result<File, JanitorError> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .mode(0o600)
        .open(path)?;
    let info = file.metadata()?;
    if !info.is_file() || info.uid() != euid() {
        return Err(JanitorError::os("unsafe cleanup lock"));
    }
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

/// Python's `exc.errno in (errno.EACCES, errno.EAGAIN)` busy test (fs2
/// reports flock contention as WouldBlock; the raw-code check keeps the
/// Python errno set exactly).
pub(crate) fn lock_contended(exc: &io::Error) -> bool {
    exc.kind() == io::ErrorKind::WouldBlock
        || matches!(exc.raw_os_error(), Some(c) if c == nix::libc::EACCES || c == nix::libc::EAGAIN)
}

/// An exclusive cleanup-run lock that always issues `LOCK_UN` before close.
///
/// The holder token makes record removal conditional. Without it, a process
/// whose old lock inode was retired could finish later and delete the current
/// holder's record merely because both records use the same pathname.
pub(crate) struct ExclusiveLock {
    pub(crate) file: File,
    pub(crate) holder_records: Vec<PathBuf>,
    pub(crate) holder_token: Option<String>,
}

impl Drop for ExclusiveLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
        let Some(token) = &self.holder_token else {
            return;
        };
        for path in &self.holder_records {
            let current_token = read_lock_holder_at(path).map(|holder| holder.token);
            if current_token.as_deref() == Some(token.as_str()) {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

/// What one holder said about itself when it took the lock.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct LockHolder {
    pub(crate) pid: i32,
    pub(crate) acquired_at: f64,
    /// Epoch seconds by which this holder expects to be finished: its pass
    /// deadline, not a guess made by the reader.
    pub(crate) deadline_at: f64,
    pub(crate) writer: String,
    pub(crate) writer_version: String,
    /// Unique ownership token. Empty only for records written by an older
    /// Stado release; those remain readable during the rolling upgrade.
    #[serde(default)]
    token: String,
}

/// Why a contended lock is contended, in the terms an operator needs.
pub(crate) enum LockState {
    /// Ours, and the record now says so.
    Held(ExclusiveLock),
    /// Somebody else holds it and is still inside their declared budget.
    Busy { holder: Option<LockHolder> },
    /// Taken from a holder that is past its own declared deadline. Carries the
    /// evidence so the pass can report it rather than looking like a normal
    /// run.
    TakenOver {
        lock: ExclusiveLock,
        from_pid: i32,
        overdue_seconds: f64,
    },
}

fn holder_record_path(state_dir: &Path) -> PathBuf {
    state_dir.join(LOCK_HOLDER_NAME)
}

pub(crate) fn holder_inode_record_path(
    state_dir: &Path,
    file: &File,
) -> Result<PathBuf, JanitorError> {
    let info = file.metadata()?;
    Ok(state_dir.join(format!(
        "{LOCK_HOLDER_INODE_PREFIX}{}.{}",
        info.dev(),
        info.ino()
    )))
}

fn read_lock_holder_at(path: &Path) -> Option<LockHolder> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(path)
        .ok()?;
    let info = file.metadata().ok()?;
    if !info.is_file() || info.uid() != euid() {
        return None;
    }
    let mut raw = String::new();
    file.read_to_string(&mut raw).ok()?;
    serde_json::from_str(&raw).ok()
}

pub(crate) fn read_lock_holder(state_dir: &Path, lock: &File) -> Option<LockHolder> {
    holder_inode_record_path(state_dir, lock)
        .ok()
        .and_then(|path| read_lock_holder_at(&path))
        .or_else(|| read_lock_holder_at(&holder_record_path(state_dir)))
}

pub(crate) fn lock_token() -> String {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{}-{nanos}", std::process::id())
}

pub(crate) fn write_lock_holder(
    state_dir: &Path,
    lock: &File,
    pass_seconds: f64,
    writer: &str,
) -> Result<(String, Vec<PathBuf>), JanitorError> {
    let now = epoch_now();
    let token = lock_token();
    let record = LockHolder {
        pid: std::process::id() as i32,
        acquired_at: now,
        deadline_at: now + pass_seconds,
        writer: writer.to_string(),
        writer_version: env!("CARGO_PKG_VERSION").to_string(),
        token: token.clone(),
    };
    let body = serde_json::to_vec(&record)?;
    let records = vec![
        holder_inode_record_path(state_dir, lock)?,
        holder_record_path(state_dir),
    ];
    let mut written = Vec::new();
    for destination in &records {
        if let Ok(info) = std::fs::symlink_metadata(destination) {
            if info.file_type().is_symlink() || !info.is_file() || info.uid() != euid() {
                for path in &written {
                    let _ = std::fs::remove_file(path);
                }
                return Err(JanitorError::os("unsafe cleanup lock holder record"));
            }
        }
        let file_name = destination
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| JanitorError::os("invalid cleanup lock holder record path"))?;
        let staged = state_dir.join(format!(".{file_name}.{token}"));
        let result = (|| -> Result<(), JanitorError> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .custom_flags(nix::libc::O_NOFOLLOW)
                .mode(0o600)
                .open(&staged)?;
            file.write_all(&body)?;
            file.sync_data()?;
            std::fs::rename(&staged, destination)?;
            Ok(())
        })();
        if let Err(error) = result {
            let _ = std::fs::remove_file(&staged);
            for path in &written {
                let _ = std::fs::remove_file(path);
            }
            return Err(error);
        }
        written.push(destination.clone());
    }
    Ok((token, records))
}

/// Map an io error to the Python `type(exc).__name__` style the agent logs.
pub(crate) fn io_code(exc: &io::Error) -> &'static str {
    JanitorError::from(io::Error::from_raw_os_error(
        exc.raw_os_error().unwrap_or(0),
    ))
    .code
}
