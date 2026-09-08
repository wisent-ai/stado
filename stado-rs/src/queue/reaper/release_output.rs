//! The one exception to "a stale lease means retry": a release worker that
//! already published a complete, self-consistent result before its lease
//! lapsed. Everything here is evidence checking, not a transition.

use chrono::Utc;

use crate::models::Job;
use crate::monitor::heartbeat_guard as hg;
use crate::queue::{JobStorage, StorageError};

/// Return the completion timestamp when this stale job has a complete,
/// self-consistent release-worker result in canonical storage.
///
/// The bootstrap writes the canonical archive and receipt only after the
/// release worker exits successfully. The receipt is still not trusted by
/// itself: its immutable request is read through the job's resolved input,
/// both digests are checked, and every identity field must agree. Requiring
/// the receipt timestamp to belong to this exact execution prevents output
/// retained from an earlier lease-expiry retry from completing a newer one.
pub(super) async fn verified_release_completion(
    store: &JobStorage,
    job: &Job,
    now: chrono::DateTime<Utc>,
    log: &dyn Fn(&str),
) -> Result<Option<String>, StorageError> {
    let receipt_path = format!("status/{}/output/receipt.json", job.job_id);
    let Some(receipt_bytes) = store.read_bytes(&receipt_path).await? else {
        return Ok(None);
    };
    let receipt: crate::release_pipeline::BuildReceipt =
        match serde_json::from_slice(&receipt_bytes) {
            Ok(receipt) => receipt,
            Err(error) => {
                log(&format!(
                    "{}: retained output is not a release receipt: {error}",
                    job.job_id
                ));
                return Ok(None);
            }
        };
    let Some(request_input) = job
        .resolved_input_artifacts
        .get("request")
        .and_then(serde_json::Value::as_object)
    else {
        return Ok(None);
    };
    if request_input
        .get("relative_path")
        .and_then(serde_json::Value::as_str)
        != Some("release-request.json")
    {
        return Ok(None);
    }
    let Some(request_uri) = request_input
        .get("stado_uri")
        .and_then(serde_json::Value::as_str)
    else {
        return Ok(None);
    };
    let Some(request_sha256) = request_input
        .get("sha256")
        .and_then(serde_json::Value::as_str)
    else {
        return Ok(None);
    };
    let request_object = match crate::object_store::ObjectRef::parse(request_uri) {
        Ok(object) => object,
        Err(error) => {
            log(&format!(
                "{}: release request URI is invalid: {error}",
                job.job_id
            ));
            return Ok(None);
        }
    };
    let configured_namespace = crate::config::wc_stado_storage_namespace();
    if !configured_namespace.is_empty() && request_object.namespace() != configured_namespace {
        log(&format!(
            "{}: release request namespace {} differs from queue namespace {}",
            job.job_id,
            request_object.namespace(),
            configured_namespace
        ));
        return Ok(None);
    }
    let request_path = store.backend().blob_path(&request_object);
    let Some(request_bytes) = store.read_bytes(&request_path).await? else {
        log(&format!(
            "{}: release request disappeared from {request_uri}",
            job.job_id
        ));
        return Ok(None);
    };
    if crate::release_control::sha256_bytes(&request_bytes) != request_sha256 {
        log(&format!(
            "{}: release request digest disagrees with its immutable job input",
            job.job_id
        ));
        return Ok(None);
    }
    let request: crate::release_pipeline::WorkerRequest =
        match serde_json::from_slice(&request_bytes) {
            Ok(request) => request,
            Err(error) => {
                log(&format!(
                    "{}: immutable release request is invalid: {error}",
                    job.job_id
                ));
                return Ok(None);
            }
        };
    let completed = match hg::parse_iso_lenient(&receipt.completed_at) {
        Some(completed) => completed,
        None => {
            log(&format!(
                "{}: release receipt has an invalid completion timestamp",
                job.job_id
            ));
            return Ok(None);
        }
    };
    let started = job
        .started_at
        .as_deref()
        .filter(|value| !value.is_empty())
        .and_then(hg::parse_iso_lenient);
    let inputs_match = receipt.inputs.len() == request.inputs.len()
        && receipt.inputs.iter().all(|(name, input)| {
            request.inputs.get(name).is_some_and(|expected| {
                input.uri == expected.uri
                    && input.sha256 == expected.sha256
                    && input.mount == expected.mount
                    && input.extract == expected.extract
            })
        });
    let identity_matches = receipt.schema_version == 1
        && request.schema_version == 1
        && receipt.run_id == request.run_id
        && receipt.job_id == job.job_id
        && receipt.product == request.product
        && receipt.version == request.version
        && receipt.platform == request.platform
        && receipt.builder == request.builder
        && receipt.source_commit == request.source_commit
        && receipt.source_sha256 == request.source_sha256
        && receipt.manifest_sha256 == request.manifest_sha256
        && receipt.secret_env == request.secret_env
        && inputs_match
        && receipt.status == crate::release_pipeline::StepStatus::Passed
        && receipt.build.status == crate::release_pipeline::StepStatus::Passed
        && receipt.build.exit_code == Some(0)
        && receipt.failure.is_none()
        && receipt.quality.iter().all(|step| {
            step.status == crate::release_pipeline::StepStatus::Passed && step.exit_code == Some(0)
        })
        && started.is_some_and(|started| completed >= started)
        && completed <= now;
    if !identity_matches {
        log(&format!(
            "{}: retained release receipt does not match this execution's immutable request",
            job.job_id
        ));
        return Ok(None);
    }
    let Some(artifact) = receipt.artifact.as_ref() else {
        log(&format!(
            "{}: passed release receipt omitted its artifact",
            job.job_id
        ));
        return Ok(None);
    };
    if artifact.path != "release.tar.gz" {
        log(&format!(
            "{}: release receipt names unexpected artifact path {:?}",
            job.job_id, artifact.path
        ));
        return Ok(None);
    }
    let archive_path = format!("status/{}/output/release.tar.gz", job.job_id);
    let Some(archive) = store.read_bytes(&archive_path).await? else {
        log(&format!(
            "{}: passed release receipt has no canonical archive",
            job.job_id
        ));
        return Ok(None);
    };
    if u64::try_from(archive.len()).ok() != Some(artifact.bytes)
        || crate::release_control::sha256_bytes(&archive) != artifact.sha256
    {
        log(&format!(
            "{}: canonical release archive disagrees with its receipt",
            job.job_id
        ));
        return Ok(None);
    }
    Ok(Some(receipt.completed_at))
}
