//! The [`BlobBackend`] surface of the device-local filesystem store.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::queue::{json_str, BlobBackend, BlobInfo, StorageError, VersionedText};

use super::paths::remove_missing_ok;
use super::LocalBackend;

#[async_trait]
impl BlobBackend for LocalBackend {
    async fn upload_text(&self, path: &str, content: &str) -> Result<(), StorageError> {
        let target = self.path(path)?;
        self.atomic_write(&target, content.as_bytes())
    }

    async fn upload_bytes(&self, path: &str, content: &[u8]) -> Result<(), StorageError> {
        let target = self.path(path)?;
        self.atomic_write(&target, content)
    }

    async fn download_text(&self, path: &str) -> Result<Option<String>, StorageError> {
        let Some(data) = self.download_bytes(path).await? else {
            return Ok(None);
        };
        // Python `.decode()` raises on invalid UTF-8.
        String::from_utf8(data)
            .map(Some)
            .map_err(|err| StorageError::Other(format!("invalid UTF-8 in {path}: {err}")))
    }

    async fn download_bytes(&self, path: &str) -> Result<Option<Vec<u8>>, StorageError> {
        let target = self.path(path)?;
        match fs::read(&target) {
            Ok(data) => Ok(Some(data)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    async fn download_to_filename(&self, path: &str, dest: &Path) -> Result<bool, StorageError> {
        let source = self.path(path)?;
        if !source.is_file() {
            return Ok(false);
        }
        // Python `shutil.copyfile` overwrites `dest`.
        fs::copy(&source, dest)?;
        Ok(true)
    }

    async fn upload_text_if_absent(&self, path: &str, content: &str) -> Result<bool, StorageError> {
        self.create_if_absent(path, content.as_bytes())
    }

    async fn upload_file_if_absent(
        &self,
        path: &str,
        local_file: &Path,
    ) -> Result<bool, StorageError> {
        let data = fs::read(local_file)?;
        self.create_if_absent(path, &data)
    }

    async fn download_text_versioned(
        &self,
        path: &str,
    ) -> Result<Option<VersionedText>, StorageError> {
        let target = self.path(path)?;
        self.with_lock(path, || {
            if !target.is_file() {
                return Ok(None);
            }
            let data = fs::read(&target)?;
            let content = String::from_utf8(data)
                .map_err(|err| StorageError::Other(format!("invalid UTF-8 in {path}: {err}")))?;
            Ok(Some(VersionedText {
                version: Self::version(content.as_bytes()),
                content,
            }))
        })
    }

    async fn compare_and_swap_text(
        &self,
        path: &str,
        expected_version: &str,
        content: &str,
    ) -> Result<String, StorageError> {
        let target = self.path(path)?;
        self.with_lock(path, || {
            if !target.is_file() {
                // Python raises FileNotFoundError here.
                return Err(StorageError::NotFound(path.to_string()));
            }
            let current = fs::read(&target)?;
            if Self::version(&current) != expected_version {
                return Err(StorageError::StorageConflict(format!(
                    "local storage version changed for {path}"
                )));
            }
            let data = content.as_bytes();
            self.atomic_write(&target, data)?;
            Ok(Self::version(data))
        })
    }

    async fn delete(&self, path: &str) -> Result<(), StorageError> {
        let _write_guard = self.mutation_guard()?;
        remove_missing_ok(&self.path(path)?)?;
        remove_missing_ok(&self.metadata_path(path))
    }

    async fn exists(&self, path: &str) -> Result<bool, StorageError> {
        Ok(self.path(path)?.is_file())
    }

    async fn list_paths(
        &self,
        prefix: &str,
        oldest_first: usize,
    ) -> Result<Vec<String>, StorageError> {
        let mut paths = self.relative_names(prefix)?;
        if oldest_first > 0 {
            // Python sorts by st_ctime (inode change time) ascending.
            let ctime = |value: &String| -> (i64, i64) {
                use std::os::unix::fs::MetadataExt;
                self.root
                    .join(value)
                    .metadata()
                    .map(|md| (md.ctime(), md.ctime_nsec()))
                    .unwrap_or_default()
            };
            paths.sort_by(|a, b| ctime(a).cmp(&ctime(b)).then_with(|| a.cmp(b)));
            paths.truncate(oldest_first);
            Ok(paths)
        } else {
            paths.sort();
            Ok(paths)
        }
    }

    /// A filesystem has no server-side cursor, so the win here is narrower
    /// than for a bucket backend and worth stating plainly: the page is
    /// produced by one prefix-scoped directory walk and one sort, with no
    /// per-name metadata lookup at all. The trait default would route through
    /// `list_paths`, which sorts the prefix and then hands it back to be
    /// sorted and scanned a second time; and the ordering it would inherit is
    /// only incidentally lexicographic, because `list_paths`' other branch
    /// orders by ctime and pays a `stat` per candidate to do it — the cost
    /// that made one diagnostics listing walk 27k queue blobs. Name order is
    /// this method's contract, so it is derived from the names alone.
    async fn list_page(
        &self,
        prefix: &str,
        start_after: &str,
        limit: usize,
    ) -> Result<Vec<String>, StorageError> {
        let mut names = self.relative_names(prefix)?;
        names.sort_unstable();
        let cut = names.partition_point(|name| name.as_str() <= start_after);
        names.drain(..cut);
        if limit > 0 {
            names.truncate(limit);
        }
        Ok(names)
    }

    async fn updated_at(&self, path: &str) -> Result<Option<DateTime<Utc>>, StorageError> {
        match self.path(path)?.metadata() {
            Ok(md) => Ok(Some(DateTime::<Utc>::from(md.modified()?))),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    async fn set_metadata(
        &self,
        path: &str,
        kv: &BTreeMap<String, String>,
    ) -> Result<(), StorageError> {
        if !self.path(path)?.is_file() {
            return Ok(());
        }
        let metadata_path = self.metadata_path(path);
        // Tolerate a missing or corrupt sidecar by starting from empty.
        let mut current: BTreeMap<String, String> = fs::read_to_string(&metadata_path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        // Python skips None and empty-string values.
        current.extend(
            kv.iter()
                .filter(|(_, value)| !value.is_empty())
                .map(|(k, v)| (k.clone(), v.clone())),
        );
        // Python `json.dumps(current, sort_keys=True)` with default
        // separators (", " / ": "). BTreeMap iterates in sorted key order.
        let body = current
            .iter()
            .map(|(k, v)| format!("{}: {}", json_str(k), json_str(v)))
            .collect::<Vec<_>>()
            .join(", ");
        self.atomic_write(&metadata_path, format!("{{{body}}}").as_bytes())
    }

    async fn list_blobs_with_meta(&self, prefix: &str) -> Result<Vec<BlobInfo>, StorageError> {
        let mut out = Vec::new();
        for path in self.list_paths(prefix, 0).await? {
            let metadata: BTreeMap<String, String> = fs::read_to_string(self.metadata_path(&path))
                .ok()
                .and_then(|raw| serde_json::from_str(&raw).ok())
                .unwrap_or_default();
            let size = fs::metadata(self.path(&path)?)
                .ok()
                .map(|entry| entry.len());
            out.push(BlobInfo {
                updated: self.updated_at(&path).await?,
                name: path,
                size,
                metadata,
            });
        }
        Ok(out)
    }
}
