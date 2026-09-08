//! Storage-root write fence: the advisory lock and durable intent that refuse
//! local mutations while an A/B storage handoff owns the deployment root.

use std::fs::{self, File, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use nix::libc;

use crate::queue::StorageError;

use super::paths::{local_io, normalize};
use super::LocalBackend;

#[derive(Debug)]
pub(super) struct WriteFencePaths {
    lock: PathBuf,
    intent: PathBuf,
}

impl WriteFencePaths {
    pub(super) fn for_root(root: &Path) -> Option<Self> {
        let pair = root.ancestors().find(|path| {
            matches!(
                path.file_name().and_then(|name| name.to_str()),
                Some("local-storage" | "local-backup")
            ) && path
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|name| name == ".stado")
        })?;
        let recovery = pair.parent()?.join("recovery");
        Some(Self {
            lock: recovery.join("storage-root-writes.lock"),
            intent: recovery.join("storage-root-write-fence.json"),
        })
    }

    fn open_lock(&self) -> Result<File, StorageError> {
        if let Some(parent) = self.lock.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| local_io("create write-fence directory", parent, error))?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&self.lock)
            .map_err(|error| local_io("open write-fence lock", &self.lock, error))?;
        if !file.metadata()?.is_file() {
            return Err(StorageError::Other(format!(
                "local storage write-fence lock is not a regular file: {}",
                self.lock.display()
            )));
        }
        Ok(file)
    }

    fn read_intent(&self) -> Result<Option<serde_json::Value>, StorageError> {
        let file = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&self.intent)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(local_io("read write-fence intent", &self.intent, error)),
        };
        if !file.metadata()?.is_file() {
            return Err(StorageError::Other(format!(
                "local storage write-fence intent is not a regular file: {}",
                self.intent.display()
            )));
        }
        let intent: serde_json::Value = serde_json::from_reader(file)?;
        if intent.get("schema").and_then(serde_json::Value::as_str)
            != Some(LocalBackend::WRITE_FENCE_PROTOCOL)
            || intent
                .get("transaction")
                .and_then(serde_json::Value::as_str)
                .is_none_or(str::is_empty)
        {
            return Err(StorageError::Other(format!(
                "local storage write-fence intent is invalid: {}",
                self.intent.display()
            )));
        }
        Ok(Some(intent))
    }

    fn refused(&self, intent: Option<&serde_json::Value>) -> StorageError {
        let owner = intent
            .and_then(|value| value.get("transaction"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("an active storage handoff");
        std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            format!(
                "local storage writes are fenced by {owner}; inspect the recorded storage-root-reconcile transaction"
            ),
        )
        .into()
    }

    pub(super) fn mutation_guard(&self) -> Result<File, StorageError> {
        let file = self.open_lock()?;
        match fs2::FileExt::try_lock_shared(&file) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                return Err(self.refused(self.read_intent()?.as_ref()));
            }
            Err(error) => return Err(local_io("acquire shared write fence", &self.lock, error)),
        }
        // The durable intent also refuses writes after an interrupted owner
        // has lost its descriptor. An in-flight writer holding this shared
        // lock must finish before the owner can acquire its exclusive hold.
        if let Some(intent) = self.read_intent()? {
            return Err(self.refused(Some(&intent)));
        }
        Ok(file)
    }
}

impl LocalBackend {
    pub(crate) const WRITE_FENCE_PROTOCOL: &'static str = "stado.storage-root-write-fence.v1";

    pub(crate) fn write_fence_paths(root: &Path) -> Option<(PathBuf, PathBuf)> {
        WriteFencePaths::for_root(root).map(|paths| (paths.lock, paths.intent))
    }

    pub(crate) fn write_guard_for_root(root: &Path) -> Result<Option<File>, StorageError> {
        WriteFencePaths::for_root(root)
            .map(|paths| paths.mutation_guard())
            .transpose()
    }

    pub(crate) fn open_write_fence_lock(root: &Path) -> Result<File, StorageError> {
        WriteFencePaths::for_root(root)
            .ok_or_else(|| {
                StorageError::Other(format!(
                    "local storage root is outside the A/B handoff scope: {}",
                    root.display()
                ))
            })?
            .open_lock()
    }

    pub(crate) fn resolved_root(root: &str) -> Result<PathBuf, StorageError> {
        let expanded = crate::config_file::expand_tilde(root);
        let absolute = if expanded.is_absolute() {
            expanded
        } else {
            std::env::current_dir()?.join(expanded)
        };
        Ok(normalize(&absolute))
    }

    pub(crate) fn write_fence_state(root: &Path) -> Result<serde_json::Value, StorageError> {
        let root = Self::resolved_root(root.to_str().ok_or_else(|| {
            StorageError::Other("local storage root is not valid UTF-8".to_string())
        })?)?;
        let Some(paths) = WriteFencePaths::for_root(&root) else {
            return Ok(serde_json::json!({"protocol": null, "intent": null}));
        };
        Ok(serde_json::json!({
            "protocol": Self::WRITE_FENCE_PROTOCOL,
            "root": root,
            "lock": paths.lock,
            "intent_path": paths.intent,
            "intent": paths.read_intent()?,
        }))
    }

    pub(super) fn mutation_guard(&self) -> Result<Option<File>, StorageError> {
        self.write_fence
            .as_ref()
            .map(WriteFencePaths::mutation_guard)
            .transpose()
    }
}
