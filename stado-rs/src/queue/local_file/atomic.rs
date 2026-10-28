//! Whole-or-absent publication: create-if-absent, tempfile/fsync/rename
//! writes, and the per-blob advisory lock the CAS path runs under.

use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::queue::StorageError;

use super::paths::{local_io, off_thread};
use super::LocalBackend;

/// How a whole blob takes its place: over what is there, or only where
/// nothing is.
#[derive(Clone, Copy)]
enum Placing {
    Replace,
    IfAbsent,
}

/// [`publish`] over whatever `target` holds, for a step already running on
/// the blocking pool under the write fence.
pub(super) fn publish_replacing(target: &Path, data: &[u8]) -> Result<(), StorageError> {
    publish(target, data, Placing::Replace).map(|_| ())
}

/// Put `data` at `target` whole and durably through the one writer Stado
/// keeps its files with (`primitives::document`): staged beside it, flushed,
/// placed by rename (`Replace`) or by a link the kernel refuses when `target`
/// exists (`IfAbsent`), and the directory flushed. `false` only when
/// `IfAbsent` found `target` already there. Owner-only, as the queue's
/// records always were.
fn publish(target: &Path, data: &[u8], placing: Placing) -> Result<bool, StorageError> {
    let mode = crate::primitives::file_mode::owner_read_write();
    match placing {
        Placing::Replace => {
            crate::primitives::document::persist_with_mode(target, data, mode).map(|()| true)
        }
        Placing::IfAbsent => crate::primitives::document::persist_if_absent(target, data, mode),
    }
    .map_err(|error| local_io("persist", target, error))
}

impl LocalBackend {
    /// Publish `path` only if absent, and only whole: the bytes are written to a
    /// sibling temporary file, fsynced, and then linked into place. `link` fails
    /// with `EEXIST` when the target exists, so the create race is still decided by
    /// the kernel and the answer is still "did we win".
    ///
    /// Creating the target first and writing into it afterwards — which is what
    /// this did — left a window where a crash produced an object that exists and is
    /// truncated. For a mutable blob that is repairable. For a release object it is
    /// not: create-only means the coordinate can never be rewritten, so a partial
    /// object would burn that version permanently. Either the object is complete or
    /// it is absent, and absent is republishable.
    ///
    /// The write fence is taken here; the write and its fsyncs run on the
    /// runtime's blocking pool holding it, off the thread that awaits them.
    pub(super) async fn create_if_absent(
        &self,
        path: &str,
        data: Vec<u8>,
    ) -> Result<bool, StorageError> {
        let write_guard = self.mutation_guard()?;
        let target = self.path(path)?;
        off_thread(path, move || {
            let _write_guard = write_guard;
            publish(&target, &data, Placing::IfAbsent)
        })
        .await
    }

    /// Write via tempfile in the target directory + fsync + rename, so a
    /// concurrent reader never observes a partial blob. The write fence is
    /// taken here; the write and its fsyncs run on the runtime's blocking
    /// pool holding it: a part upload's fsync on a loaded disk took seconds on
    /// the thread that serves every object API request.
    pub(super) async fn atomic_write(
        &self,
        path: &str,
        target: PathBuf,
        data: Vec<u8>,
    ) -> Result<(), StorageError> {
        let write_guard = self.mutation_guard()?;
        off_thread(path, move || {
            let _write_guard = write_guard;
            publish_replacing(&target, &data)
        })
        .await
    }

    /// Replace `path` with `content` when it still holds the version
    /// `expected`, under the blob's exclusive lock, answering the new
    /// version. The fence is taken on the calling thread; waiting for the
    /// lock, reading, comparing and writing run on the blocking pool, so a
    /// writer holding the lock never holds the thread that serves requests.
    pub(super) async fn swap_if_version(
        &self,
        path: &str,
        expected: String,
        content: Vec<u8>,
    ) -> Result<String, StorageError> {
        let write_guard = self.mutation_guard()?;
        let target = self.path(path)?;
        let lock = self.lock_path(path);
        let name = path.to_owned();
        off_thread(path, move || {
            let _write_guard = write_guard;
            locked(&lock, &name, || {
                if !target.is_file() {
                    // Python raises FileNotFoundError here.
                    return Err(StorageError::NotFound(name.clone()));
                }
                let current = fs::read(&target)?;
                if Self::version(&current) != expected {
                    return Err(StorageError::StorageConflict(format!(
                        "local storage version changed for {name}"
                    )));
                }
                publish_replacing(&target, &content)?;
                Ok(Self::version(&content))
            })
        })
        .await
    }

    /// `path`'s text and its version, read under the blob's exclusive lock
    /// so no swap lands between the two; `None` when there is no such blob.
    /// Waiting for the lock and the read run on the blocking pool.
    pub(super) async fn read_versioned(
        &self,
        path: &str,
    ) -> Result<Option<(String, String)>, StorageError> {
        let target = self.path(path)?;
        let lock = self.lock_path(path);
        let name = path.to_owned();
        off_thread(path, move || {
            locked(&lock, &name, || {
                if !target.is_file() {
                    return Ok(None);
                }
                let data = fs::read(&target)?;
                let version = Self::version(&data);
                let content = String::from_utf8(data).map_err(|err| {
                    StorageError::Other(format!("invalid UTF-8 in {name}: {err}"))
                })?;
                Ok(Some((content, version)))
            })
        })
        .await
    }

    /// The lock file of one blob (Python `fcntl.flock(LOCK_EX)` on
    /// `.locks/<sha256(path)>`).
    fn lock_path(&self, path: &str) -> PathBuf {
        self.locks
            .join(hex::encode(Sha256::digest(path.as_bytes())))
    }
}

/// Run `f` holding the exclusive flock at `lock_path`, which guards the blob
/// `path`; the wait for it is announced. Called on the blocking pool.
fn locked<T>(
    lock_path: &Path,
    path: &str,
    f: impl FnOnce() -> Result<T, StorageError>,
) -> Result<T, StorageError> {
    // Python opens the lock file "a+b" (read + append).
    let file = OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .open(lock_path)
        .map_err(|error| local_io("open lock", lock_path, error))?;
    crate::wait::blocking(
        crate::wait::Kind::Lock,
        format!("exclusive lock on the local store's {path}"),
        lock_path.display(),
        || file.lock(),
    )?;
    let result = f();
    // Closing the descriptor releases the lock; there is no separate unlock
    // call whose failure could be dropped.
    drop(file);
    result
}
