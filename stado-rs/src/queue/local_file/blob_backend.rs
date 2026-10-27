//! The [`BlobBackend`] surface of the device-local filesystem store.

use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::num::NonZeroUsize;
use std::path::Path;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::queue::{json_str, BlobBackend, BlobInfo, Listing, StorageError, VersionedText};

use super::atomic::publish_replacing;
use super::paths::{modified, off_thread, read_range, read_sidecar, remove_missing_ok};
use super::LocalBackend;

#[async_trait]
impl BlobBackend for LocalBackend {
    async fn upload_text(&self, path: &str, content: &str) -> Result<(), StorageError> {
        let target = self.path(path)?;
        self.atomic_write(path, target, content.as_bytes().to_vec())
            .await
    }

    async fn upload_bytes(&self, path: &str, content: &[u8]) -> Result<(), StorageError> {
        let target = self.path(path)?;
        self.atomic_write(path, target, content.to_vec()).await
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

    /// An object's bytes, read on the runtime's blocking pool: a release
    /// archive is gigabytes, and reading it on the thread that serves the
    /// object API held every other request for as long as the read took.
    async fn download_bytes(&self, path: &str) -> Result<Option<Vec<u8>>, StorageError> {
        let target = self.path(path)?;
        off_thread(path, move || match fs::read(&target) {
            Ok(data) => Ok(Some(data)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err.into()),
        })
        .await
    }

    /// One byte range, read from its offset on the blocking pool.
    async fn read_range(
        &self,
        path: &str,
        first: u64,
        last: u64,
    ) -> Result<Option<Vec<u8>>, StorageError> {
        let target = self.path(path)?;
        off_thread(path, move || read_range(&target, first, last)).await
    }

    async fn download_to_filename(&self, path: &str, dest: &Path) -> Result<bool, StorageError> {
        let source = self.path(path)?;
        let dest = dest.to_path_buf();
        off_thread(path, move || {
            if !source.is_file() {
                return Ok(false);
            }
            // Python `shutil.copyfile` overwrites `dest`.
            fs::copy(&source, &dest)?;
            Ok(true)
        })
        .await
    }

    async fn upload_text_if_absent(&self, path: &str, content: &str) -> Result<bool, StorageError> {
        self.create_if_absent(path, content.as_bytes().to_vec())
            .await
    }

    async fn upload_file_if_absent(
        &self,
        path: &str,
        local_file: &Path,
    ) -> Result<bool, StorageError> {
        let source = local_file.to_path_buf();
        let data = off_thread(path, move || fs::read(&source).map_err(StorageError::from)).await?;
        self.create_if_absent(path, data).await
    }

    async fn download_text_versioned(
        &self,
        path: &str,
    ) -> Result<Option<VersionedText>, StorageError> {
        Ok(self
            .read_versioned(path)
            .await?
            .map(|(content, version)| VersionedText { version, content }))
    }

    async fn compare_and_swap_text(
        &self,
        path: &str,
        expected_version: &str,
        content: &str,
    ) -> Result<String, StorageError> {
        self.swap_if_version(
            path,
            expected_version.to_owned(),
            content.as_bytes().to_vec(),
        )
        .await
    }

    async fn delete(&self, path: &str) -> Result<(), StorageError> {
        let write_guard = self.mutation_guard()?;
        let target = self.path(path)?;
        let metadata = self.metadata_path(path);
        off_thread(path, move || {
            let _write_guard = write_guard;
            remove_missing_ok(&target)?;
            remove_missing_ok(&metadata)
        })
        .await
    }

    async fn exists(&self, path: &str) -> Result<bool, StorageError> {
        let target = self.path(path)?;
        off_thread(path, move || Ok(target.is_file())).await
    }

    async fn list_paths(
        &self,
        prefix: &str,
        listing: Listing,
    ) -> Result<Vec<String>, StorageError> {
        let mut paths = self.relative_names(prefix).await?;
        match listing {
            Listing::Oldest(count) => {
                // Oldest by inode change time, names breaking ties. A name
                // removed since the walk saw it is no longer under the
                // prefix; any other metadata failure is refused.
                let root = self.root.clone();
                off_thread(prefix, move || {
                    use std::os::unix::fs::MetadataExt;
                    let mut dated = Vec::with_capacity(paths.len());
                    for name in paths {
                        match root.join(&name).metadata() {
                            Ok(found) => dated.push(((found.ctime(), found.ctime_nsec()), name)),
                            Err(error) if error.kind() == ErrorKind::NotFound => {}
                            Err(error) => return Err(error.into()),
                        }
                    }
                    dated.sort();
                    dated.truncate(count.get());
                    Ok(dated.into_iter().map(|(_, name)| name).collect())
                })
                .await
            }
            Listing::Every => {
                paths.sort();
                Ok(paths)
            }
        }
    }

    /// A filesystem has no server-side cursor, so the win here is narrower
    /// than for a bucket backend and worth stating plainly: the page is
    /// produced by one prefix-scoped directory walk and one sort, with no
    /// per-name metadata lookup at all. The trait default would route through
    /// `list_paths`, which sorts the prefix and then hands it back to be
    /// sorted and scanned a second time; and the ordering it would inherit is
    /// only incidentally lexicographic, because `list_paths`' other branch
    /// orders by ctime and pays a `stat` per candidate. Name order is this
    /// method's contract, so it is derived from the names alone.
    async fn list_page(
        &self,
        prefix: &str,
        start_after: &str,
        limit: Option<NonZeroUsize>,
    ) -> Result<Vec<String>, StorageError> {
        let mut names = self.relative_names(prefix).await?;
        names.sort_unstable();
        let cut = names.partition_point(|name| name.as_str() <= start_after);
        names.drain(..cut);
        if let Some(limit) = limit {
            names.truncate(limit.get());
        }
        Ok(names)
    }

    async fn updated_at(&self, path: &str) -> Result<Option<DateTime<Utc>>, StorageError> {
        let target = self.path(path)?;
        off_thread(path, move || modified(&target)).await
    }

    /// One blob's sidecar metadata, size and modification time, read on the
    /// blocking pool from one `stat`, or `None` when no file has that name.
    async fn stat(&self, path: &str) -> Result<Option<BlobInfo>, StorageError> {
        let target = self.path(path)?;
        let sidecar = self.metadata_path(path);
        let name = path.to_string();
        off_thread(path, move || {
            let entry = match fs::metadata(&target) {
                Ok(entry) => entry,
                Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error.into()),
            };
            if !entry.is_file() {
                return Ok(None);
            }
            Ok(Some(BlobInfo {
                metadata: read_sidecar(&sidecar)?,
                updated: Some(DateTime::<Utc>::from(entry.modified()?)),
                size: Some(entry.len()),
                name,
            }))
        })
        .await
    }

    async fn set_metadata(
        &self,
        path: &str,
        kv: &BTreeMap<String, String>,
    ) -> Result<(), StorageError> {
        let write_guard = self.mutation_guard()?;
        let target = self.path(path)?;
        let metadata_path = self.metadata_path(path);
        // Python skips None and empty-string values.
        let stated: Vec<(String, String)> = kv
            .iter()
            .filter(|(_, value)| !value.is_empty())
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        off_thread(path, move || {
            let _write_guard = write_guard;
            if !target.is_file() {
                return Ok(());
            }
            let mut current = read_sidecar(&metadata_path)?;
            current.extend(stated);
            // Python `json.dumps(current, sort_keys=True)` with default
            // separators (", " / ": "). BTreeMap iterates in sorted key order.
            let body = current
                .iter()
                .map(|(k, v)| format!("{}: {}", json_str(k), json_str(v)))
                .collect::<Vec<_>>()
                .join(", ");
            publish_replacing(&metadata_path, format!("{{{body}}}").as_bytes())
        })
        .await
    }

    /// Each blob under `prefix` with its sidecar metadata, size and
    /// modification time, read on the blocking pool from one `stat` per blob.
    async fn list_blobs_with_meta(&self, prefix: &str) -> Result<Vec<BlobInfo>, StorageError> {
        let names = self.list_paths(prefix, Listing::Every).await?;
        let mut located = Vec::with_capacity(names.len());
        for name in names {
            located.push((self.path(&name)?, self.metadata_path(&name), name));
        }
        off_thread(prefix, move || {
            let mut out = Vec::with_capacity(located.len());
            for (target, sidecar, name) in located {
                let metadata = read_sidecar(&sidecar)?;
                // A blob removed since the listing has neither size nor change
                // time; any other failure to read it is refused.
                let (size, updated) = match fs::metadata(&target) {
                    Ok(entry) => (
                        Some(entry.len()),
                        Some(DateTime::<Utc>::from(entry.modified()?)),
                    ),
                    Err(error) if error.kind() == ErrorKind::NotFound => (None, None),
                    Err(error) => return Err(error.into()),
                };
                out.push(BlobInfo {
                    updated,
                    name,
                    size,
                    metadata,
                });
            }
            Ok(out)
        })
        .await
    }

    fn watch_prefixes(
        &self,
        prefixes: &[&str],
    ) -> Result<Box<dyn crate::queue::ChangeWatch>, StorageError> {
        self.arm_watch(prefixes)
    }
}
