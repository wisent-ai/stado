//! Bounded artifact collection out of the Box and into the job's status
//! prefix, with the containment check every requested path passes.
//!
//! Port of `stado/scheduler/dispatch/box/output.py`.

use crate::models::Job;
use crate::providers::r#box::BoxClient;

use super::super::runtime::Keepalive;
use super::super::BoxDispatchError;
use super::{ARTIFACT_BYTES, ARTIFACT_COUNT};

/// Python `_safe_artifact_path` (ValueError).
fn safe_artifact_path(value: &str) -> Result<String, BoxDispatchError> {
    let path = value.trim();
    // PurePosixPath semantics: absolute paths and any ".." part are
    // rejected; repeated slashes / "." parts normalize away.
    let bad = path.is_empty() || path.starts_with('/') || path.split('/').any(|part| part == "..");
    if bad {
        return Err(BoxDispatchError::value(
            "Box artifact path must be relative and contained",
        ));
    }
    Ok(path.to_string())
}

/// Python `upload_artifacts`: bounded artifact collection into status/.
pub(crate) async fn upload_artifacts(
    store: &crate::queue::JobStorage,
    client: &BoxClient,
    job: &Job,
    box_id: &str,
    keepalive: &mut Keepalive<'_, '_>,
) -> Result<(), BoxDispatchError> {
    if job.artifact_paths.len() > ARTIFACT_COUNT {
        return Err(BoxDispatchError::value("too many Box artifacts requested"));
    }
    let mut remaining = ARTIFACT_BYTES;
    for source in &job.artifact_paths {
        keepalive.ping().await?;
        let path = safe_artifact_path(source)?;
        if remaining == 0 {
            return Err(BoxDispatchError::value(
                "Box artifact aggregate byte bound exceeded",
            ));
        }
        let content = client.download_artifact(box_id, &path, remaining).await?;
        remaining -= content.len();
        let destination = format!(
            "status/{}/output/artifacts/{}",
            job.job_id,
            path.replace('/', "_")
        );
        store.upload_bytes(&destination, &content).await?;
        keepalive.ping().await?;
    }
    Ok(())
}
