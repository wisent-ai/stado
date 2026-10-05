//! Bounded artifact collection out of the Box and into the job's status
//! prefix, with the containment check every requested path passes.
//!
//! Port of `stado/scheduler/dispatch/box/output.py`.

use crate::models::Job;
use crate::providers::r#box::BoxClient;

use super::super::runtime::Keepalive;
use super::super::BoxDispatchError;

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

/// Python `upload_artifacts`: every requested artifact into status/.
pub(crate) async fn upload_artifacts(
    store: &crate::queue::JobStorage,
    client: &BoxClient,
    job: &Job,
    box_id: &str,
    keepalive: &mut Keepalive<'_, '_>,
) -> Result<(), BoxDispatchError> {
    for source in &job.artifact_paths {
        keepalive.ping().await?;
        let path = safe_artifact_path(source)?;
        let content = client.download_artifact(box_id, &path).await?;
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
