//! Filesystem-backed job storage for a device-local Stado deployment.
//!
//! Port of `stado/queue/local_file.py` (`LocalFileBackend`). Implements the
//! blob backend contract using atomic local files: blobs are plain files at
//! `{root}/{path}`, metadata sidecars live under `.metadata/`, advisory
//! locks under `.locks/` (flock via fs2), writes go through a tempfile +
//! fsync + rename, if-absent creation uses `O_CREAT|O_EXCL`, and the CAS
//! version token is the SHA-256 hex of the content.
//!
//! This backend intentionally serves one device. Remote workers require a
//! cloud-backed deployment instead of exposing this directory over the
//! network.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use fs2::FileExt;
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

use super::{json_str, BlobBackend, BlobInfo, StorageError, VersionedText};

/// Local filesystem implementation of [`BlobBackend`].
#[derive(Debug)]
pub struct LocalBackend {
    root: PathBuf,
    locks: PathBuf,
    metadata: PathBuf,
}

impl LocalBackend {
    /// Root the backend at `root` (created when missing, `~` expanded).
    /// Python raises `RuntimeError` for an empty `WC_LOCAL_STORAGE_PATH`.
    pub fn new(root: &str) -> Result<Self, StorageError> {
        if root.is_empty() {
            return Err(StorageError::Other(
                "WC_LOCAL_STORAGE_PATH is required for local storage".into(),
            ));
        }
        let expanded = crate::config_file::expand_tilde(root);
        // Python `Path(root).expanduser().resolve()`: anchor relative paths
        // at the cwd and normalize. Unlike Python resolve() this does NOT
        // follow symlinks — lexical normalization only (non-strict resolve
        // semantics for paths that may not exist yet).
        let absolute = if expanded.is_absolute() {
            expanded
        } else {
            std::env::current_dir()?.join(expanded)
        };
        let root = normalize(&absolute);
        fs::create_dir_all(&root)?;
        let locks = root.join(".locks");
        let metadata = root.join(".metadata");
        fs::create_dir_all(&locks)?;
        fs::create_dir_all(&metadata)?;
        Ok(Self {
            root,
            locks,
            metadata,
        })
    }

    /// Resolve a blob path against the deployment root, rejecting escapes
    /// (Python `ValueError("storage path escapes deployment root")`).
    fn path(&self, path: &str) -> Result<PathBuf, StorageError> {
        let target = normalize(&self.root.join(path));
        if !target.starts_with(&self.root) {
            return Err(StorageError::PathEscape(path.to_string()));
        }
        Ok(target)
    }

    fn metadata_path(&self, path: &str) -> PathBuf {
        if Path::new(path)
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            self.metadata.join(path)
        } else {
            self.metadata.join(format!("{path}.json"))
        }
    }

    /// Whether a filesystem path lives under `.locks/` or `.metadata/`
    /// (excluded from listings).
    fn is_internal(&self, path: &Path) -> bool {
        path.starts_with(&self.locks) || path.starts_with(&self.metadata)
    }

    /// SHA-256 hex version token for CAS.
    fn version(data: &[u8]) -> String {
        hex::encode(Sha256::digest(data))
    }

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
    fn create_if_absent(&self, path: &str, data: &[u8]) -> Result<bool, StorageError> {
        let target = self.path(path)?;
        let parent = target.parent().ok_or_else(|| {
            StorageError::Other(format!("no parent directory for {}", target.display()))
        })?;
        fs::create_dir_all(parent)?;
        let name = target.file_name().unwrap_or_default().to_string_lossy();
        let prefix = format!(".{}.", name.trim_start_matches('.'));
        let mut tmp = NamedTempFile::with_prefix_in(prefix, parent)?;
        tmp.write_all(data)?;
        tmp.as_file().sync_all()?;
        match tmp.persist_noclobber(&target) {
            Ok(_) => Ok(true),
            Err(err) if err.error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
            Err(err) => Err(err.error.into()),
        }
    }

    /// Write via tempfile in the target directory + fsync + rename, so a
    /// concurrent reader never observes a partial blob.
    fn atomic_write(&self, target: &Path, data: &[u8]) -> Result<(), StorageError> {
        let parent = target.parent().ok_or_else(|| {
            StorageError::Other(format!("no parent directory for {}", target.display()))
        })?;
        fs::create_dir_all(parent)?;
        let name = target.file_name().unwrap_or_default().to_string_lossy();
        let prefix = format!(".{}.", name.trim_start_matches('.'));
        let mut tmp = NamedTempFile::with_prefix_in(prefix, parent)?;
        tmp.write_all(data)?;
        tmp.as_file().sync_all()?;
        tmp.persist(target).map_err(|err| err.error)?;
        Ok(())
    }

    /// Run `f` under an exclusive flock keyed on the blob path (Python
    /// `fcntl.flock(LOCK_EX)` on `.locks/<sha256(path)>`).
    fn with_lock<T>(
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
            .open(lock_path)?;
        file.lock_exclusive()?;
        let result = f();
        // Python releases the lock in a finally block and would propagate an
        // unlock failure; flock release failure is not actionable here.
        let _ = file.unlock();
        result
    }
}

/// Lexical path normalization: resolve `.` and `..` without touching the
/// filesystem (Python `Path.resolve(strict=False)` on POSIX, minus symlink
/// resolution).
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Recursively collect files under `dir`.
fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out)?;
        } else if path.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

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
        let mut files = Vec::new();
        walk(&self.root, &mut files)?;
        let mut paths: Vec<String> = files
            .into_iter()
            .filter(|item| !self.is_internal(item))
            .filter_map(|item| {
                item.strip_prefix(&self.root).ok().map(|rel| {
                    rel.components()
                        .map(|c| c.as_os_str().to_string_lossy().into_owned())
                        .collect::<Vec<_>>()
                        .join("/")
                })
            })
            .filter(|rel| rel.starts_with(prefix))
            .collect();
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
        if let Some(parent) = metadata_path.parent() {
            fs::create_dir_all(parent)?;
        }
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

/// Python `Path.unlink(missing_ok=True)`.
fn remove_missing_ok(path: &Path) -> Result<(), StorageError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn backend() -> (tempfile::TempDir, Arc<LocalBackend>) {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(LocalBackend::new(dir.path().to_str().unwrap()).unwrap());
        (dir, backend)
    }

    #[tokio::test]
    async fn upload_download_round_trip_and_missing() {
        let (_dir, b) = backend();
        assert_eq!(b.download_text("a/b.txt").await.unwrap(), None);
        assert_eq!(b.download_bytes("a/b.txt").await.unwrap(), None);
        b.upload_text("a/b.txt", "hello").await.unwrap();
        assert_eq!(
            b.download_text("a/b.txt").await.unwrap().as_deref(),
            Some("hello")
        );
        assert_eq!(
            b.download_bytes("a/b.txt").await.unwrap().as_deref(),
            Some(b"hello".as_slice())
        );
        assert!(b.exists("a/b.txt").await.unwrap());
        // Unconditional overwrite.
        b.upload_text("a/b.txt", "world").await.unwrap();
        assert_eq!(
            b.download_text("a/b.txt").await.unwrap().as_deref(),
            Some("world")
        );
    }

    #[tokio::test]
    async fn download_to_filename_copies_and_reports_absence() {
        let (dir, b) = backend();
        let dest = dir.path().join("out.bin");
        assert!(!b.download_to_filename("nope", &dest).await.unwrap());
        b.upload_text("blob", "data").await.unwrap();
        assert!(b.download_to_filename("blob", &dest).await.unwrap());
        assert_eq!(fs::read_to_string(&dest).unwrap(), "data");
    }

    #[tokio::test]
    async fn upload_text_if_absent_is_atomic_create() {
        let (_dir, b) = backend();
        assert!(b.upload_text_if_absent("lock", "first").await.unwrap());
        assert!(!b.upload_text_if_absent("lock", "second").await.unwrap());
        assert_eq!(
            b.download_text("lock").await.unwrap().as_deref(),
            Some("first")
        );
    }

    #[tokio::test]
    async fn upload_file_if_absent_uploads_once() {
        let (dir, b) = backend();
        let src = dir.path().join("src.txt");
        fs::write(&src, "file-bytes").unwrap();
        assert!(b.upload_file_if_absent("f", &src).await.unwrap());
        fs::write(&src, "changed").unwrap();
        assert!(!b.upload_file_if_absent("f", &src).await.unwrap());
        assert_eq!(
            b.download_text("f").await.unwrap().as_deref(),
            Some("file-bytes")
        );
    }

    #[tokio::test]
    async fn versioned_read_and_cas() {
        let (_dir, b) = backend();
        assert_eq!(b.download_text_versioned("cas").await.unwrap(), None);
        b.upload_text("cas", "one").await.unwrap();
        let v1 = b.download_text_versioned("cas").await.unwrap().unwrap();
        assert_eq!(v1.content, "one");
        assert_eq!(v1.version, hex::encode(Sha256::digest(b"one")));

        // Winning CAS returns the new version and updates the content.
        let v2 = b
            .compare_and_swap_text("cas", &v1.version, "two")
            .await
            .unwrap();
        assert_eq!(v2, hex::encode(Sha256::digest(b"two")));
        assert_eq!(
            b.download_text("cas").await.unwrap().as_deref(),
            Some("two")
        );

        // Stale version loses the race.
        let err = b
            .compare_and_swap_text("cas", &v1.version, "three")
            .await
            .unwrap_err();
        assert!(matches!(err, StorageError::StorageConflict(_)), "{err:?}");

        // CAS against a missing blob: Python raises FileNotFoundError.
        let err = b.compare_and_swap_text("gone", "x", "y").await.unwrap_err();
        assert!(matches!(err, StorageError::NotFound(_)), "{err:?}");
    }

    #[tokio::test]
    async fn delete_is_idempotent_and_clears_sidecar() {
        let (_dir, b) = backend();
        b.upload_text("d", "x").await.unwrap();
        b.set_metadata("d", &BTreeMap::from([("k".into(), "v".into())]))
            .await
            .unwrap();
        b.delete("d").await.unwrap();
        assert!(!b.exists("d").await.unwrap());
        assert!(b.list_blobs_with_meta("d").await.unwrap().is_empty());
        // Second delete must not fail.
        b.delete("d").await.unwrap();
        b.delete("never-existed").await.unwrap();
    }

    #[tokio::test]
    async fn list_paths_sorted_prefixed_and_bounded() {
        let (_dir, b) = backend();
        b.upload_text("pref/b.json", "1").await.unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        b.upload_text("pref/a.json", "2").await.unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        b.upload_text("pref/c.json", "3").await.unwrap();
        b.upload_text("other/x.json", "4").await.unwrap();
        // Metadata sidecars must not leak into listings.
        b.set_metadata("pref/a.json", &BTreeMap::from([("k".into(), "v".into())]))
            .await
            .unwrap();

        assert_eq!(
            b.list_paths("pref/", 0).await.unwrap(),
            vec!["pref/a.json", "pref/b.json", "pref/c.json"]
        );
        // Bounded listing picks the N oldest by creation time.
        let oldest = b.list_paths("pref/", 2).await.unwrap();
        assert_eq!(oldest, vec!["pref/b.json", "pref/a.json"]);
    }

    #[tokio::test]
    async fn updated_at_and_metadata_merge() {
        let (_dir, b) = backend();
        assert_eq!(b.updated_at("m").await.unwrap(), None);
        b.upload_text("m", "x").await.unwrap();
        let updated = b.updated_at("m").await.unwrap().unwrap();
        assert!((Utc::now() - updated).num_seconds() < 60);

        // set_metadata on a missing blob is a no-op.
        b.set_metadata("missing", &BTreeMap::from([("k".into(), "v".into())]))
            .await
            .unwrap();
        assert!(!b.exists("missing").await.unwrap());

        // Merge semantics; empty values are skipped (Python parity).
        b.set_metadata("m", &BTreeMap::from([("a".into(), "1".into())]))
            .await
            .unwrap();
        b.set_metadata(
            "m",
            &BTreeMap::from([("b".into(), "2".into()), ("empty".into(), String::new())]),
        )
        .await
        .unwrap();
        let infos = b.list_blobs_with_meta("m").await.unwrap();
        assert_eq!(infos.len(), 1);
        assert_eq!(
            infos[0].metadata,
            BTreeMap::from([("a".into(), "1".into()), ("b".into(), "2".into())])
        );
        assert!(infos[0].updated.is_some());
    }

    #[tokio::test]
    async fn path_escape_is_rejected() {
        let (_dir, b) = backend();
        let err = b.upload_text("../evil", "x").await.unwrap_err();
        assert!(matches!(err, StorageError::PathEscape(_)), "{err:?}");
    }

    #[test]
    fn empty_root_is_an_error() {
        let err = LocalBackend::new("").unwrap_err();
        assert!(err.to_string().contains("WC_LOCAL_STORAGE_PATH"));
    }
}
