use chrono::{DateTime, Utc};
use serde_json::{json, Map, Value};

use crate::models::Job;
use crate::queue::capacity::read_consumer_capacity;
use crate::queue::JobStorage;

use super::{HandlerError, HandlerResult};

const ACTIONS: &[&str] = &[
    "jobs.snapshot",
    "jobs.timeseries",
    "fleet.host-health",
    "fleet.registry",
];

pub(super) fn supports(action: &str) -> bool {
    ACTIONS.contains(&action)
}

/// Every action takes the empty object; any other body is a bad request, and
/// why is logged as an upstream read's cause is.
fn empty_request(body: &[u8]) -> Result<(), HandlerError> {
    match serde_json::from_slice::<Value>(body) {
        Ok(value) if value == json!({}) => Ok(()),
        Ok(_) => {
            tracing::warn!("integration request body is JSON but not the empty object");
            Err(HandlerError::BadRequest)
        }
        Err(error) => {
            tracing::warn!(cause = %error, "integration request body is not JSON");
            Err(HandlerError::BadRequest)
        }
    }
}

/// When `job` reached the state `stamp` records; `None` when it records no
/// time. A stamp that is not a time refuses the read with the job and the
/// value, since read as absent it put the job at the end of "newest first".
fn timestamp(job: &Job, stamp: Option<&str>) -> Result<Option<DateTime<Utc>>, HandlerError> {
    let Some(value) = stamp else {
        return Ok(None);
    };
    match DateTime::parse_from_rfc3339(value) {
        Ok(parsed) => Ok(Some(parsed.with_timezone(&Utc))),
        Err(error) => Err(HandlerError::upstream(format!(
            "job {} records {value:?}, which is not a time: {error}",
            job.job_id
        ))),
    }
}

/// `jobs` newest first by the stamp `stamp` reads from each.
fn newest_first(
    jobs: &[Job],
    stamp: impl Fn(&Job) -> Option<&str>,
) -> Result<Vec<Job>, HandlerError> {
    let mut dated = jobs
        .iter()
        .map(|job| -> Result<_, HandlerError> { Ok((timestamp(job, stamp(job))?, job.clone())) })
        .collect::<Result<Vec<_>, HandlerError>>()?;
    dated.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    Ok(dated.into_iter().map(|(_, job)| job).collect())
}

/// Every job as JSON. A job that does not serialize is refused rather than
/// left out, so the snapshot never answers fewer jobs than its counts.
fn jobs_value(jobs: impl IntoIterator<Item = Job>) -> Result<Value, HandlerError> {
    jobs.into_iter()
        .map(|job| serde_json::to_value(job).map_err(HandlerError::upstream))
        .collect::<Result<Vec<_>, _>>()
        .map(Value::Array)
}

/// Every job in each lifecycle state. The listing holds every state, empty
/// or not; a state it lacks is a listing this read cannot use, not a state
/// with no jobs.
async fn jobs_by_state(
    store: &JobStorage,
) -> Result<impl Fn(&str) -> Result<Vec<Job>, HandlerError>, HandlerError> {
    let all = store
        .list_all_jobs()
        .await
        .map_err(HandlerError::upstream)?;
    Ok(move |name: &str| {
        all.get(name)
            .cloned()
            .ok_or_else(|| HandlerError::upstream(format!("the job listing holds no {name} state")))
    })
}

async fn snapshot(store: &JobStorage) -> HandlerResult {
    let state = jobs_by_state(store).await?;
    let queue = state(crate::queue::runs::QUEUE)?;
    let running = state(crate::queue::runs::RUNNING)?;
    let completed = state(crate::queue::runs::COMPLETED)?;
    let failed = state(crate::queue::runs::FAILED)?;
    let recent_completed = newest_first(&completed, |job| job.completed_at.as_deref())?;
    let recent_failed = newest_first(&failed, |job| job.failed_at.as_deref())?;
    let capacities = read_consumer_capacity(store)
        .await
        .map_err(HandlerError::upstream)?
        .into_values()
        .collect::<Vec<_>>();
    // Every job in each state, newest first: the reader decides how much of
    // it to show.
    Ok(json!({
        "counts": {
            "queued": queue.len(),
            "running": running.len(),
            "completed": completed.len(),
            "failed": failed.len(),
        },
        "queue": jobs_value(queue)?,
        "running": jobs_value(running)?,
        "completed": jobs_value(recent_completed)?,
        "failed": jobs_value(recent_failed)?,
        "capacities": capacities,
    }))
}

async fn timeseries(store: &JobStorage) -> HandlerResult {
    let state = jobs_by_state(store).await?;
    Ok(json!({
        "completed": jobs_value(state(crate::queue::runs::COMPLETED)?)?,
        "failed": jobs_value(state(crate::queue::runs::FAILED)?)?,
    }))
}

async fn read_json_objects(store: &JobStorage, prefix: &str) -> Result<Vec<Value>, HandlerError> {
    let blobs = store
        .list_blobs_with_meta(prefix)
        .await
        .map_err(HandlerError::upstream)?;
    let mut values = Vec::new();
    for blob in blobs
        .into_iter()
        .filter(|blob| blob.name.ends_with(".json"))
    {
        let Some(raw) = store
            .download_text(&blob.name)
            .await
            .map_err(HandlerError::upstream)?
        else {
            continue;
        };
        let mut value: Value = serde_json::from_str(&raw).map_err(|error| {
            HandlerError::upstream(format!("{} is not JSON: {error}", blob.name))
        })?;
        if let Some(object) = value.as_object_mut() {
            if let Some(updated) = blob.updated {
                object.insert("fileUpdatedAt".into(), Value::String(updated.to_rfc3339()));
            }
        }
        values.push(value);
    }
    Ok(values)
}

async fn host_health(store: &JobStorage) -> HandlerResult {
    let beacon_prefix = crate::monitor::host_health::beacon_object_path("")
        .trim_end_matches(".json")
        .to_string();
    let mut hosts = read_json_objects(store, &beacon_prefix).await?;
    let explicit = hosts
        .iter()
        .filter_map(|value| value.get("host").and_then(Value::as_str))
        .map(str::to_string)
        .collect::<std::collections::BTreeSet<_>>();
    for capacity in read_consumer_capacity(store)
        .await
        .map_err(HandlerError::upstream)?
        .into_values()
    {
        let Some(host) = capacity.get("consumer_id").and_then(Value::as_str) else {
            continue;
        };
        if explicit.contains(host) {
            continue;
        }
        // A capacity broadcast is proof of life only as of its own
        // `published_at`. One without it says nothing about when the host was
        // alive, so its unit is `unknown` with the reason and `reported_at` is
        // null, instead of `active` beside an empty timestamp. A broadcast
        // counts no restarts, so `n_restarts` is null.
        let published_at = capacity.get("published_at").and_then(Value::as_str);
        let unit = match published_at {
            Some(published_at) => json!({
                "state": crate::deploy::service::STATE_ACTIVE,
                "n_restarts": Value::Null,
                "active_since": published_at,
            }),
            None => json!({
                "state": crate::deploy::service::STATE_UNKNOWN,
                "detail": "the live Stado capacity broadcast carries no published_at",
            }),
        };
        let mut units = Map::new();
        units.insert("wisent-agent".into(), unit);
        // Capacity says the host is alive; it says nothing about its disk,
        // so no disk figures are made up for it.
        hosts.push(json!({
            "host": host,
            "reported_at": published_at,
            "units": units,
            "last_log": "live Stado capacity broadcast",
        }));
    }
    Ok(json!({"hosts": hosts}))
}

async fn registry(store: &JobStorage) -> HandlerResult {
    let beacons = read_json_objects(store, "install_status/").await?;
    // The same projection the retired dashboard snapshot cached: one record
    // per artifact manifest, computed on demand for this authenticated
    // caller instead of by a background refresh loop nobody else reads.
    let registry = crate::artifacts::registry::ArtifactRegistry::with_store(store.clone());
    let manifests = registry
        .list("", "", "", &[])
        .await
        .map_err(HandlerError::upstream)?;
    let mut artifacts = Vec::with_capacity(manifests.len());
    for manifest in manifests {
        // The primary location's URI; null for a manifest that names none.
        let primary = manifest
            .locations
            .iter()
            .find(|location| location.role == "primary")
            .map(|location| location.uri.clone());
        let aliases = registry
            .aliases_for(&manifest.ref_)
            .await
            .map_err(HandlerError::upstream)?;
        artifacts.push(json!({
            "ref": manifest.ref_.to_string(),
            "title": manifest.title,
            "aliases": aliases,
            "verification": manifest.verification.result,
            "run_id": manifest.producer.run_id,
            "primary_uri": primary,
            "summary": manifest.summary,
            "created_at": manifest.created_at,
        }));
    }
    Ok(json!({"beacons": beacons, "artifacts": artifacts}))
}

pub(super) async fn handle(action: &str, body: &[u8], store: &JobStorage) -> HandlerResult {
    empty_request(body)?;
    match action {
        "jobs.snapshot" => snapshot(store).await,
        "jobs.timeseries" => timeseries(store).await,
        "fleet.host-health" => host_health(store).await,
        "fleet.registry" => registry(store).await,
        _ => Err(HandlerError::BadRequest),
    }
}
