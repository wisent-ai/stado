//! The lock file itself, its holder record, and the exclusive hold.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::providers::local::space_reclaim::janitor::pass::lock::euid;
use crate::providers::local::space_reclaim::janitor::state::error::JanitorError;
use crate::providers::local::space_reclaim::janitor::state::report::build::epoch_now;
use crate::providers::local::space_reclaim::janitor::{
    LOCK_HOLDER_INODE_PREFIX, LOCK_HOLDER_NAME, LOCK_NAME,
};

/// Read and write for the owner alone, the mode every lock and holder record
/// is created with and forced to, from the platform's `<sys/stat.h>` bits.
const OWNER_READ_WRITE: u32 =
    crate::primitives::file_mode::widen(nix::libc::S_IRUSR | nix::libc::S_IWUSR);

/// Open the run lock ([`LOCK_NAME`]) with O_RDWR|O_CREAT|
/// O_NOFOLLOW, verify it is a regular file owned by us, force
/// [`OWNER_READ_WRITE`].
pub(crate) fn open_lock(state_dir: &Path) -> Result<File, JanitorError> {
    open_lock_at(&state_dir.join(LOCK_NAME))
}

/// The same checks at an exact path: the state file's own lock beside it.
pub(crate) fn open_lock_at(path: &Path) -> Result<File, JanitorError> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .mode(OWNER_READ_WRITE)
        .open(path)?;
    let info = file.metadata()?;
    if !info.is_file() || info.uid() != euid() {
        return Err(JanitorError::os("unsafe cleanup lock"));
    }
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(std::fs::Permissions::from_mode(OWNER_READ_WRITE))?;
    Ok(file)
}

/// An exclusive cleanup-run lock that always issues `LOCK_UN` before close.
///
/// The holder token makes record removal conditional. Without it, a process
/// whose old lock inode was retired could finish later and delete the current
/// holder's record merely because both records use the same pathname.
pub(crate) struct ExclusiveLock {
    pub(crate) file: File,
    pub(crate) holder_records: Vec<PathBuf>,
    pub(crate) holder_token: String,
}

impl Drop for ExclusiveLock {
    fn drop(&mut self) {
        for path in &self.holder_records {
            let current_token = match read_lock_holder_at(path) {
                Ok(holder) => holder.map(|holder| holder.token),
                Err(error) => {
                    eprintln!("janitor: a cleanup lock holder record stays unread: {error}");
                    None
                }
            };
            if current_token.as_deref() == Some(self.holder_token.as_str()) {
                discard(path);
            }
        }
        if let Err(error) = self.file.unlock() {
            eprintln!("janitor: the cleanup lock could not be unlocked: {error}");
        }
    }
}

/// Remove a holder record or its staging file. One that stays names a holder
/// that no longer holds the lock, so that is said with its path and cause.
fn discard(path: &std::path::Path) {
    if let Some(left) = crate::primitives::staging::remove(path) {
        eprintln!("janitor: a cleanup lock holder record stays: {left}");
    }
}

/// What one holder said about itself when it took the lock.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct LockHolder {
    pub(crate) pid: i32,
    pub(crate) acquired_at: f64,
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
    /// Somebody else holds the kernel lock, regardless of its age: the
    /// holder's record, none when it wrote none, or why it could not be read.
    Busy {
        holder: Result<Option<LockHolder>, String>,
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

/// One holder record: `None` when there is none. A record that is not a
/// regular file owned by this user, cannot be read, or does not parse is the
/// error with its path and cause; it was the same `None`, and the operator
/// read "no readable holder record" with nothing saying why.
fn read_lock_holder_at(path: &Path) -> Result<Option<LockHolder>, String> {
    let mut file = match OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot open {}: {error}", path.display())),
    };
    let info = file
        .metadata()
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
    if !info.is_file() || info.uid() != euid() {
        return Err(format!(
            "{} is not a regular file owned by this user",
            path.display()
        ));
    }
    let mut raw = String::new();
    file.read_to_string(&mut raw)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    serde_json::from_str(&raw)
        .map(Some)
        .map_err(|error| format!("{} is not a lock holder record: {error}", path.display()))
}

/// The holder record of `lock`: its inode's own record, else the shared one.
pub(crate) fn read_lock_holder(
    state_dir: &Path,
    lock: &File,
) -> Result<Option<LockHolder>, String> {
    let inode = holder_inode_record_path(state_dir, lock).map_err(|error| error.to_string())?;
    match read_lock_holder_at(&inode)? {
        Some(holder) => Ok(Some(holder)),
        None => read_lock_holder_at(&holder_record_path(state_dir)),
    }
}

/// A token no other holder carries: this process and the clock in nanos. A
/// clock before the Unix epoch is refused, as a token from it would repeat.
fn lock_token() -> Result<String, JanitorError> {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|error| {
            JanitorError::os(&format!(
                "the clock stands before the Unix epoch, so no lock holder token can be made: \
                 {error}"
            ))
        })?
        .as_nanos();
    Ok(format!("{}-{nanos}", std::process::id()))
}

pub(crate) fn write_lock_holder(
    state_dir: &Path,
    lock: &File,
    writer: &str,
) -> Result<(String, Vec<PathBuf>), JanitorError> {
    let now = epoch_now();
    let token = lock_token()?;
    let record = LockHolder {
        pid: std::process::id() as i32,
        acquired_at: now,
        writer: writer.to_string(),
        writer_version: env!("CARGO_PKG_VERSION").to_string(),
        token: token.clone(),
    };
    let body = serde_json::to_vec(&record)?;
    let records = vec![
        holder_inode_record_path(state_dir, lock)?,
        holder_record_path(state_dir),
    ];
    let mut written: Vec<PathBuf> = Vec::new();
    for destination in &records {
        // A record that is there must be a regular file of ours; one that is
        // not there is written, and one that cannot be inspected is refused
        // with its cause rather than overwritten unseen.
        match std::fs::symlink_metadata(destination) {
            Ok(info)
                if info.file_type().is_symlink() || !info.is_file() || info.uid() != euid() =>
            {
                for path in &written {
                    discard(path);
                }
                return Err(JanitorError::os("unsafe cleanup lock holder record"));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                for path in &written {
                    discard(path);
                }
                return Err(error.into());
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
                .mode(OWNER_READ_WRITE)
                .open(&staged)?;
            file.write_all(&body)?;
            file.sync_data()?;
            std::fs::rename(&staged, destination)?;
            Ok(())
        })();
        if let Err(error) = result {
            discard(&staged);
            for path in &written {
                discard(path);
            }
            return Err(error);
        }
        written.push(destination.clone());
    }
    Ok((token, records))
}

/// The Python `type(exc).__name__` style the agent logs for an io error,
/// read from the error's own kind.
pub(crate) fn io_code(exc: &io::Error) -> &'static str {
    JanitorError::from(io::Error::from(exc.kind())).code
}
