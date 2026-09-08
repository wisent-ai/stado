//! Whole-or-absent publication: create-if-absent, tempfile/fsync/rename
//! writes, and the per-blob advisory lock the CAS path runs under.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;

use fs2::FileExt;
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

use crate::queue::StorageError;

use super::paths::local_io;
use super::LocalBackend;

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
    /// The temporary file is created 0600 by `tempfile`, matching what the previous
    /// explicit mode requested.
    pub(super) fn create_if_absent(&self, path: &str, data: &[u8]) -> Result<bool, StorageError> {
        let _write_guard = self.mutation_guard()?;
        let target = self.path(path)?;
        let parent = target.parent().ok_or_else(|| {
            StorageError::Other(format!("no parent directory for {}", target.display()))
        })?;
        fs::create_dir_all(parent)
            .map_err(|error| local_io("create parent directory for", &target, error))?;
        let name = target.file_name().unwrap_or_default().to_string_lossy();
        let prefix = format!(".{}.", name.trim_start_matches('.'));
        let mut tmp = NamedTempFile::with_prefix_in(prefix, parent)
            .map_err(|error| local_io("create temporary file for", &target, error))?;
        tmp.write_all(data)
            .map_err(|error| local_io("write temporary file for", &target, error))?;
        tmp.as_file()
            .sync_all()
            .map_err(|error| local_io("sync temporary file for", &target, error))?;
        match tmp.persist_noclobber(&target) {
            Ok(_) => {
                File::open(parent)
                    .and_then(|directory| directory.sync_all())
                    .map_err(|error| local_io("sync parent directory for", &target, error))?;
                Ok(true)
            }
            Err(err) if err.error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
            Err(err) => Err(local_io("persist", &target, err.error)),
        }
    }

    /// Write via tempfile in the target directory + fsync + rename, so a
    /// concurrent reader never observes a partial blob.
    pub(super) fn atomic_write(&self, target: &Path, data: &[u8]) -> Result<(), StorageError> {
        let _write_guard = self.mutation_guard()?;
        let parent = target.parent().ok_or_else(|| {
            StorageError::Other(format!("no parent directory for {}", target.display()))
        })?;
        fs::create_dir_all(parent)
            .map_err(|error| local_io("create parent directory for", target, error))?;
        let name = target.file_name().unwrap_or_default().to_string_lossy();
        let prefix = format!(".{}.", name.trim_start_matches('.'));
        let mut tmp = NamedTempFile::with_prefix_in(prefix, parent)
            .map_err(|error| local_io("create temporary file for", target, error))?;
        tmp.write_all(data)
            .map_err(|error| local_io("write temporary file for", target, error))?;
        tmp.as_file()
            .sync_all()
            .map_err(|error| local_io("sync temporary file for", target, error))?;
        tmp.persist(target)
            .map_err(|error| local_io("persist", target, error.error))?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| local_io("sync parent directory for", target, error))?;
        Ok(())
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
        file.lock_exclusive()?;
        let result = f();
        // Python releases the lock in a finally block and would propagate an
        // unlock failure; flock release failure is not actionable here.
        let _ = file.unlock();
        result
    }
}
