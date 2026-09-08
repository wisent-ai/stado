//! Canonical output persistence: every regular file under the job's output
//! directory, and the log tail a failure record quotes, with declared secret
//! values replaced in memory before the bytes cross the storage boundary.

use super::*;

// ---------------------------------------------------------------------------
// output upload + log tail
// ---------------------------------------------------------------------------

/// All regular files under `dir`, recursively (Python `Path.rglob("*")`
/// filtered to `is_file`). Order is readdir order, like Python's scandir
/// order.
pub(super) fn walk_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk_files(&path));
        } else if path.is_file() {
            out.push(path);
        }
    }
    out
}

pub(crate) async fn output_redactions(job: &Job) -> Result<Vec<Vec<u8>>, StorageError> {
    Ok(resolve_job_secret_environment(job)
        .await?
        .into_values()
        .filter(|value| !value.is_empty())
        .map(String::into_bytes)
        .collect())
}

pub(crate) fn redact_secret_bytes(bytes: &mut [u8], secrets: &[Vec<u8>]) {
    for secret in secrets.iter().filter(|secret| !secret.is_empty()) {
        let mut offset = usize::default();
        while offset <= bytes.len().saturating_sub(secret.len()) {
            let Some(relative) = bytes[offset..]
                .windows(secret.len())
                .position(|window| window == secret.as_slice())
            else {
                break;
            };
            let start = offset + relative;
            let end = start + secret.len();
            bytes[start..end].fill(b'*');
            offset = end;
        }
    }
}

pub(crate) async fn redacted_tail(
    job: &Job,
    path: &Path,
    max_bytes: u64,
) -> Result<String, StorageError> {
    let mut file = match tokio::fs::File::open(path).await {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(error) => return Err(error.into()),
    };
    let length = file.metadata().await?.len();
    file.seek(std::io::SeekFrom::Start(length.saturating_sub(max_bytes)))
        .await?;
    let mut bytes = Vec::with_capacity(length.min(max_bytes) as usize);
    file.read_to_end(&mut bytes).await?;
    let secrets = output_redactions(job).await?;
    redact_secret_bytes(&mut bytes, &secrets);
    Ok(String::from_utf8_lossy(&bytes).trim().to_string())
}

/// Upload every regular file under `output_dir` to
/// `status/<job_id>/output/`. Secret values are replaced in memory before
/// bytes cross the durable storage boundary. Backend failures propagate; the
/// lifecycle caller decides whether to retry finalization or continue.
pub async fn upload_output(
    store: &JobStorage,
    job: &Job,
    output_dir: &Path,
) -> Result<(), StorageError> {
    if !output_dir.exists() {
        return Ok(());
    }
    let secrets = output_redactions(job).await?;
    for path in walk_files(output_dir) {
        let rel = path
            .strip_prefix(output_dir)
            .unwrap_or(&path)
            .components()
            .map(|component| component.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        let mut bytes = tokio::fs::read(&path).await?;
        redact_secret_bytes(&mut bytes, &secrets);
        store
            .upload_bytes(&format!("status/{}/output/{rel}", job.job_id), &bytes)
            .await?;
    }
    Ok(())
}
