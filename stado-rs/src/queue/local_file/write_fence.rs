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
            // Read and write by the owner alone.
            .mode(crate::primitives::file_mode::owner_read_write())
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
        // The intent names the transaction that fenced the store, and the
        // command that armed it when that was not the storage-root
        // reconcile; a fence whose intent names none is said as such, with
        // the intent's path, rather than attributed to a transaction nobody
        // recorded.
        let transaction = intent
            .and_then(|value| value.get("transaction"))
            .and_then(serde_json::Value::as_str);
        let armed_by = intent
            .and_then(|value| value.get("owner"))
            .and_then(serde_json::Value::as_str);
        let owner = match (transaction, armed_by) {
            (Some(transaction), Some(armed_by)) => format!(
                "{armed_by} transaction {transaction}; stado storage fence status names it and \
                 stado storage fence disarm --transaction {transaction} lifts it"
            ),
            (Some(transaction), None) => format!(
                "storage-root-reconcile transaction {transaction}; inspect the recorded \
                 storage-root-reconcile transaction"
            ),
            (None, _) => format!(
                "a storage handoff whose intent at {} names no transaction",
                self.intent.display()
            ),
        };
        std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            format!("local storage writes are fenced by {owner}"),
        )
        .into()
    }

    pub(super) fn mutation_guard(&self) -> Result<File, StorageError> {
        let file = self.open_lock()?;
        match file.try_lock_shared() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(self.refused(self.read_intent()?.as_ref()));
            }
            Err(std::fs::TryLockError::Error(error)) => {
                return Err(local_io("acquire shared write fence", &self.lock, error));
            }
        }
        // The durable intent also refuses writes after an interrupted owner
        // has lost its descriptor. An in-flight writer holding this shared
        // lock must finish before the owner can acquire its exclusive hold.
        // The one writer it admits is the transaction that fenced the store:
        // a role move carrying the registry authority commits its registry
        // through the store it fenced.
        if let Some(intent) = self.read_intent()? {
            if intent["transaction"].as_str() != ADMITTED.get().map(String::as_str) {
                return Err(self.refused(Some(&intent)));
            }
        }
        Ok(file)
    }
}

/// The fence transaction this process writes for, from an explicit
/// command-line input (`stado registry push --fence-transaction`).
static ADMITTED: std::sync::OnceLock<String> = std::sync::OnceLock::new();

impl LocalBackend {
    /// Admit this process's writes through a store fenced by `transaction`;
    /// a second, different transaction is refused.
    pub(crate) fn admit_fence_transaction(transaction: &str) -> Result<(), StorageError> {
        let held = ADMITTED.get_or_init(|| transaction.to_string());
        if held != transaction {
            return Err(StorageError::Other(format!(
                "this process already writes for fence transaction {held}, not {transaction}"
            )));
        }
        Ok(())
    }

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
