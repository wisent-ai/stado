use chrono::{DateTime, Duration, Utc};
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

fn empty_request(body: &[u8]) -> Result<(), HandlerError> {
    if serde_json::from_slice::<Value>(body).ok().as_ref() == Some(&json!({})) {
        Ok(())
    } else {
        Err(HandlerError::BadRequest)
    }
}

fn timestamp(job: &Job, field: &str) -> Option<DateTime<Utc>> {
    let value = match field {
        "completed" => job.completed_at.as_deref(),
        "failed" => job.failed_at.as_deref(),
        _ => None,
    }?;
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|parsed| parsed.with_timezone(&Utc))
}

fn jobs_value(jobs: impl IntoIterator<Item = Job>) -> Value {
    Value::Array(
        jobs.into_iter()
            .filter_map(|job| serde_json::to_value(job).ok())
            .collect(),
    )
}

async fn snapshot(store: &JobStorage) -> HandlerResult {
    let all = store
        .list_all_jobs()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    let queue = all.get("queue").cloned().unwrap_or_default();
    let running = all.get("running").cloned().unwrap_or_default();
    let completed = all.get("completed").cloned().unwrap_or_default();
    let failed = all.get("failed").cloned().unwrap_or_default();
    let cutoff = Utc::now() - Duration::hours("1".parse().expect("static hour"));
    let recent_completed = completed
        .iter()
        .filter(|job| timestamp(job, "completed").is_some_and(|value| value >= cutoff))
        .cloned()
        .collect::<Vec<_>>();
    let recent_failed = failed
        .iter()
        .filter(|job| timestamp(job, "failed").is_some_and(|value| value >= cutoff))
        .cloned()
        .collect::<Vec<_>>();
    let mut recent_failures = failed.clone();
    recent_failures.sort_by_key(|job| std::cmp::Reverse(timestamp(job, "failed")));
    recent_failures.truncate("50".parse().expect("static failure cap"));
    let capacities = read_consumer_capacity(store)
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?
        .into_values()
        .collect::<Vec<_>>();
    Ok(json!({
        "counts": {
            "queued": queue.len(),
            "running": running.len(),
            "completed": completed.len(),
            "failed": failed.len(),
        },
        "queueSample": jobs_value(queue.into_iter().take("200".parse().expect("static queue cap"))),
        "running": jobs_value(running),
        "recentCompleted": jobs_value(recent_completed),
        "recentFailed": jobs_value(recent_failed),
        "recentFailures": jobs_value(recent_failures),
        "capacities": capacities,
    }))
}

async fn timeseries(store: &JobStorage) -> HandlerResult {
    let cutoff = Utc::now() - Duration::hours("6".parse().expect("static horizon"));
    let completed = store
        .list_jobs("completed", usize::default())
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?
        .into_iter()
        .filter(|job| timestamp(job, "completed").is_some_and(|value| value >= cutoff));
    let failed = store
        .list_jobs("failed", usize::default())
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?
        .into_iter()
        .filter(|job| timestamp(job, "failed").is_some_and(|value| value >= cutoff));
    Ok(json!({
        "completed": jobs_value(completed),
        "failed": jobs_value(failed),
    }))
}

async fn read_json_objects(store: &JobStorage, prefix: &str) -> Result<Vec<Value>, HandlerError> {
    let blobs = store
        .list_blobs_with_meta(prefix)
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    let mut values = Vec::new();
    for blob in blobs
        .into_iter()
        .filter(|blob| blob.name.ends_with(".json"))
    {
        let Some(raw) = store
            .download_text(&blob.name)
            .await
            .map_err(|_| HandlerError::UpstreamFailure)?
        else {
            continue;
        };
        let mut value: Value =
            serde_json::from_str(&raw).map_err(|_| HandlerError::UpstreamFailure)?;
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
        .map_err(|_| HandlerError::UpstreamFailure)?
        .into_values()
    {
        let Some(host) = capacity.get("consumer_id").and_then(Value::as_str) else {
            continue;
        };
        if explicit.contains(host) {
            continue;
        }
        let reported_at = capacity
            .get("published_at")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mut units = Map::new();
        units.insert(
            "wisent-agent".into(),
            json!({"state": "active", "n_restarts": "?", "active_since": reported_at}),
        );
        hosts.push(json!({
            "host": host,
            "reported_at": reported_at,
            "disk_pct": i64::default(),
            "disk_avail_gb": i64::default(),
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
        .map_err(|_| HandlerError::UpstreamFailure)?;
    let mut artifacts = Vec::with_capacity(manifests.len());
    for manifest in manifests {
        let primary = manifest
            .locations
            .iter()
            .find(|location| location.role == "primary")
            .map(|location| location.uri.clone())
            .unwrap_or_default();
        let aliases = registry
            .aliases_for(&manifest.ref_)
            .await
            .map_err(|_| HandlerError::UpstreamFailure)?;
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
