//! Downloading a terminal job's canonical output, with every path rule the
//! Python original enforced.

use std::io::Read;
use std::path::Path;

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::machine::contract::encoding::py_repr;
use crate::machine::sources::unsafe_archive_name;
use crate::machine::{MachineError, MachineFacade};
use crate::models::job_state;

impl MachineFacade {
    /// Download and verify the canonical `status/<id>/output/` artifacts of
    /// a terminal job (Python `download_artifacts`). Every blob is hashed
    /// while streaming to disk and reported with size + sha256; output-path
    /// and storage-path symlink/escape rules are enforced exactly as Python.
    pub async fn download_artifacts(
        &self,
        job_id: &str,
        output_dir: &Path,
    ) -> Result<Value, MachineError> {
        fn security(msg: impl Into<String>) -> MachineError {
            MachineError::new("ARTIFACT_SECURITY", msg)
        }
        let job = self.lookup_job(job_id).await?;
        if !job_state::is_terminal(&job.state) {
            return Err(MachineError::new(
                "NOT_TERMINAL",
                format!("job {} is not terminal", py_repr(job_id)),
            ));
        }
        let expanded = crate::config_file::expand_tilde(&output_dir.to_string_lossy());
        // Python Path.absolute(): anchor at the cwd, no normalization.
        let requested_root = if expanded.is_absolute() {
            expanded
        } else {
            std::env::current_dir()?.join(expanded)
        };
        if requested_root.exists() && requested_root.is_symlink() {
            return Err(security("output directory must not be a symlink"));
        }
        // Symlinked ancestors are rejected, except the system temp dir and
        // ITS parents (Python's trusted_system_aliases — macOS /var -> /
        // private/var would otherwise fail every tempfile-adjacent path).
        let temp_root = std::env::temp_dir();
        let trusted: std::collections::HashSet<&Path> = temp_root.ancestors().collect();
        for component in requested_root.ancestors().skip(1) {
            if component.exists() && component.is_symlink() && !trusted.contains(component) {
                return Err(security("output path must not contain symlinks"));
            }
        }
        if requested_root.exists() && !requested_root.is_dir() {
            return Err(security("output directory path is not a directory"));
        }
        std::fs::create_dir_all(&requested_root)?;
        let root = requested_root.canonicalize()?;
        let prefix = format!("status/{job_id}/output/");
        let mut paths: Vec<String> = self
            .store
            .list_paths(&prefix, 0)
            .await?
            .into_iter()
            .filter(|path| *path != prefix)
            .collect();
        paths.sort();
        if paths.is_empty() {
            return Err(MachineError::new(
                "NO_ARTIFACTS",
                format!("job {} has no canonical output artifacts", py_repr(job_id)),
            ));
        }

        let mut artifacts: Vec<Value> = Vec::new();
        for blob_path in &paths {
            if !blob_path.starts_with(&prefix) {
                return Err(security(
                    "storage returned an artifact outside the job output prefix",
                ));
            }
            let relative = &blob_path[prefix.len()..];
            if unsafe_archive_name(relative) {
                return Err(security(format!(
                    "unsafe artifact path: {}",
                    py_repr(relative)
                )));
            }
            let parts: Vec<&str> = relative.split('/').collect();
            let mut destination = root.clone();
            for part in &parts {
                destination.push(part);
            }
            let mut current = root.clone();
            for part in &parts[..parts.len() - 1] {
                current.push(part);
                if current.exists() && (current.is_symlink() || !current.is_dir()) {
                    return Err(security(format!(
                        "unsafe output path component: {}",
                        py_repr(part)
                    )));
                }
                std::fs::create_dir(&current).or_else(|exc| {
                    if exc.kind() == std::io::ErrorKind::AlreadyExists {
                        Ok(())
                    } else {
                        Err(exc)
                    }
                })?;
            }
            if destination.exists() && (destination.is_symlink() || !destination.is_file()) {
                return Err(security(format!(
                    "unsafe artifact destination: {}",
                    py_repr(relative)
                )));
            }
            let parent = destination.parent().unwrap_or(&root).to_path_buf();
            let temporary = tempfile::Builder::new()
                .prefix(".stado-")
                .suffix(".download")
                .tempfile_in(&parent)?
                .into_temp_path();
            let download_result = async {
                let downloaded = self.store.download_blob(blob_path, &temporary).await?;
                if !downloaded {
                    return Err(MachineError::retryable(
                        "NO_ARTIFACTS",
                        format!("artifact disappeared while downloading: {relative}"),
                    ));
                }
                let mut file = std::fs::File::open(&temporary)?;
                let mut digest = Sha256::new();
                let mut size: u64 = 0;
                let mut chunk = [0u8; 1024 * 1024];
                loop {
                    let n = file.read(&mut chunk)?;
                    if n == 0 {
                        break;
                    }
                    size += n as u64;
                    digest.update(&chunk[..n]);
                }
                std::fs::rename(&temporary, &destination)?;
                Ok::<(u64, String), MachineError>((size, hex::encode(digest.finalize())))
            }
            .await;
            // Python `finally: temporary.unlink(missing_ok=True)`.
            let _ = std::fs::remove_file(&temporary);
            let (size, sha256) = download_result?;
            let mode = std::fs::symlink_metadata(&destination)?;
            if !mode.file_type().is_file() {
                return Err(security(format!(
                    "downloaded artifact is not a regular file: {}",
                    py_repr(relative)
                )));
            }
            let mut entry = Map::new();
            entry.insert("relative_path".into(), Value::from(parts.join("/")));
            entry.insert("size_bytes".into(), Value::from(size));
            entry.insert("sha256".into(), Value::from(sha256));
            artifacts.push(Value::Object(entry));
        }
        if artifacts.is_empty() {
            return Err(MachineError::new(
                "NO_ARTIFACTS",
                format!("job {} has no canonical output artifacts", py_repr(job_id)),
            ));
        }
        let mut out = Map::new();
        out.insert("job_id".into(), Value::from(job_id));
        out.insert(
            "output_dir".into(),
            Value::from(root.to_string_lossy().into_owned()),
        );
        out.insert("artifacts".into(), Value::Array(artifacts));
        Ok(Value::Object(out))
    }
}
