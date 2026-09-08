//! Filesystem-backed job storage for a device-local Stado deployment.
//!
//! Implements the shared blob contract with atomic local files. Blobs are
//! plain files under the deployment root, metadata and advisory locks use
//! private side directories, writes use tempfile/fsync/rename, conditional
//! creation uses exclusive create, and the CAS token is a content SHA-256.
//!
//! This backend intentionally serves one device. Remote workers require a
//! cloud backend rather than network exposure of this directory.

use std::fs;
use std::path::{Path, PathBuf};

use crate::queue::StorageError;

mod atomic;
mod blob_backend;
mod paths;
mod write_fence;

use paths::normalize;
use write_fence::WriteFencePaths;

/// Local filesystem implementation of [`BlobBackend`](crate::queue::BlobBackend).
#[derive(Debug)]
pub struct LocalBackend {
    root: PathBuf,
    locks: PathBuf,
    metadata: PathBuf,
    write_fence: Option<WriteFencePaths>,
}

impl LocalBackend {
    /// Root the backend at `root`, creating it when missing.
    pub fn new(root: &str) -> Result<Self, StorageError> {
        if root.is_empty() {
            return Err(StorageError::Other(
                "WC_LOCAL_STORAGE_PATH is required for local storage".into(),
            ));
        }
        let root = Self::resolved_root(root)?;
        let write_fence = WriteFencePaths::for_root(&root);
        let locks = root.join(".locks");
        let metadata = root.join(".metadata");
        if !root.is_dir() || !locks.is_dir() || !metadata.is_dir() {
            let _write_guard = write_fence
                .as_ref()
                .map(WriteFencePaths::mutation_guard)
                .transpose()?;
            fs::create_dir_all(&root)?;
            fs::create_dir_all(&locks)?;
            fs::create_dir_all(&metadata)?;
        }
        Ok(Self {
            root,
            locks,
            metadata,
            write_fence,
        })
    }

    /// Open a previously materialized local store without creating or repairing
    /// any directory. Immutable recovery snapshots use this constructor so a
    /// validator cannot turn an absent internal directory into evidence.
    pub(crate) fn open_existing(root: &Path) -> Result<Self, StorageError> {
        let root = normalize(root);
        let write_fence = WriteFencePaths::for_root(&root);
        if !root.is_dir() || root.is_symlink() {
            return Err(StorageError::Other(format!(
                "immutable local snapshot is absent or unsafe: {}",
                root.display()
            )));
        }
        let locks = root.join(".locks");
        let metadata = root.join(".metadata");
        if !locks.is_dir() || locks.is_symlink() || !metadata.is_dir() || metadata.is_symlink() {
            return Err(StorageError::Other(format!(
                "immutable local snapshot has no safe internal layout: {}",
                root.display()
            )));
        }
        Ok(Self {
            root,
            locks,
            metadata,
            write_fence,
        })
    }
}
