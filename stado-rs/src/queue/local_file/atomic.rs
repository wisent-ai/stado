//! Whole-or-absent publication: create-if-absent, tempfile/fsync/rename
//! writes, and the per-blob advisory lock the CAS path runs under.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

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

/// Write `data` to a sibling temporary file of `target`, fsync it, and put it
/// in place by rename (`Replace`) or by link (`IfAbsent`, which the kernel
/// refuses with `EEXIST` when `target` exists), then fsync the directory.
/// `false` only when `IfAbsent` found `target` already there. The temporary
/// file is created owner-only by `tempfile`.
fn publish(target: &Path, data: &[u8], placing: Placing) -> Result<bool, StorageError> {
    let parent = target.parent().ok_or_else(|| {
        StorageError::Other(format!("no parent directory for {}", target.display()))
    })?;
    let name = target.file_name().ok_or_else(|| {
        StorageError::Other(format!("{} names no file to publish", target.display()))
    })?;
    fs::create_dir_all(parent)
        .map_err(|error| local_io("create parent directory for", target, error))?;
    let prefix = format!(".{}.", name.to_string_lossy().trim_start_matches('.'));
    let mut tmp = NamedTempFile::with_prefix_in(prefix, parent)
        .map_err(|error| local_io("create temporary file for", target, error))?;
    tmp.write_all(data)
        .map_err(|error| local_io("write temporary file for", target, error))?;
    tmp.as_file()
        .sync_all()
        .map_err(|error| local_io("sync temporary file for", target, error))?;
    match placing {
        Placing::Replace => {
            tmp.persist(target)
                .map_err(|error| local_io("persist", target, error.error))?;
        }
        Placing::IfAbsent => match tmp.persist_noclobber(target) {
            Ok(_) => {}
            Err(err) if err.error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
            Err(err) => return Err(local_io("persist", target, err.error)),
        },
    }
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| local_io("sync parent directory for", target, error))?;
    Ok(true)
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
    /// concurrent reader never observes a partial blob. On the calling
    /// thread, for a write made under the per-blob lock of the CAS path.
    pub(super) fn atomic_write(&self, target: &Path, data: &[u8]) -> Result<(), StorageError> {
        let _write_guard = self.mutation_guard()?;
        publish(target, data, Placing::Replace).map(|_| ())
    }

    /// [`Self::atomic_write`] with the write and its fsyncs on the runtime's
    /// blocking pool, for an async caller: a part upload's fsync on a loaded
    /// disk took seconds on the thread that serves every object API request.
    pub(super) async fn atomic_write_off_thread(
        &self,
        path: &str,
        target: PathBuf,
        data: Vec<u8>,
    ) -> Result<(), StorageError> {
        let write_guard = self.mutation_guard()?;
        off_thread(path, move || {
            let _write_guard = write_guard;
            publish(&target, &data, Placing::Replace).map(|_| ())
        })
        .await
    }

    /// Run `f` under an exclusive flock keyed on the blob path (Python
    /// `fcntl.flock(LOCK_EX)` on `.locks/<sha256(path)>`).
    pub(super) fn with_lock<T>(
        &self,
        path: &str,
        f: impl FnOnce() -> Result<T, StorageError>,
    ) -> Result<T, StorageError> {
        let lock_path = self
            .locks
            .join(hex::encode(Sha256::digest(path.as_bytes())));
        // Python opens the lock file "a+b" (read + append).
        let file = OpenOptions::new()
            .read(true)
            .append(true)
            .create(true)
            .open(&lock_path)
            .map_err(|error| local_io("open lock", &lock_path, error))?;
        crate::wait::blocking(
            crate::wait::Kind::Lock,
            format!("exclusive lock on the local store's {path}"),
            lock_path.display(),
            || file.lock(),
        )?;
        let result = f();
        // Closing the descriptor releases the lock; there is no separate
        // unlock call whose failure could be dropped.
        drop(file);
        result
    }
}
