//! Read one run manifest document, and list every run id under `runs/`.

use serde_json::{Map, Value};

use crate::queue::storage::JobStorage;
use crate::queue::StorageError;

use super::super::prefixes::RUN_PREFIX;

/// Read a run manifest; `None` when it does not exist.
pub async fn read_run(
    store: &JobStorage,
    run_id: &str,
) -> Result<Option<Map<String, Value>>, StorageError> {
    crate::queue::submit::validate_run_id(run_id)
        .map_err(|error| StorageError::Other(error.to_string()))?;
    let path = format!("{RUN_PREFIX}/{run_id}.json");
    let Some(versioned) = store.read_text_versioned(&path).await? else {
        return Ok(None);
    };
    let value: Value = serde_json::from_str(&versioned.content)?;
    let value = if value.get("schema").and_then(Value::as_str) == Some("stado.run-submission.v2") {
        match crate::queue::submit::migrate_v2_run_manifest(store, run_id).await {
            Ok(migrated) => migrated,
            Err(crate::queue::submit::SubmitError::Storage(StorageError::NotFound(missing)))
                if missing == path =>
            {
                return Ok(None);
            }
            Err(error) => return Err(StorageError::Other(error.to_string())),
        }
    } else {
        value
    };
    match value {
        Value::Object(map) => Ok(Some(map)),
        _ => Err(StorageError::Other(format!(
            "run manifest {run_id} is not a JSON object"
        ))),
    }
}

/// Run ids of all manifests under `runs/`: each `runs/<run id>.json`.
///
/// The same prefix also holds other records in directories of their own —
/// `runs/build/<product>/<build>/manifest.json`, `runs/release-pipeline/<id>/
/// run.json`, the pending changes — and the listing is recursive. Taking the
/// last segment of every path read those leaves as run ids `manifest`, `run`
/// and `changes`, once per record, so a reader of every run (`stado job
/// watch` of a reaped job, `stado machine logs`) asked the store for
/// `runs/manifest.json` thousands of times and its connection was closed
/// before it found the job. Only a manifest directly under the prefix is a
/// run.
pub async fn list_runs(store: &JobStorage) -> Result<Vec<String>, StorageError> {
    let prefix = format!("{RUN_PREFIX}/");
    let blobs = store.list_blobs_with_meta(&prefix).await?;
    Ok(blobs
        .iter()
        .filter_map(|blob| blob.name.strip_prefix(&prefix))
        .filter(|name| !name.contains('/'))
        .filter_map(|name| name.strip_suffix(".json"))
        .map(str::to_string)
        .collect())
}

/// Written once a walk of every run has indexed each job any run retained.
/// Every retention writes its job's index entry itself, so from then on a
/// job the index does not name was retained by no run, and no reader walks
/// the runs again. It sits directly under the run prefix without the run
/// suffix, so neither a listing of runs nor one of the job index reads it.
fn walked_path() -> String {
    format!("{RUN_PREFIX}/retained-jobs-indexed")
}

/// Every job id one run manifest retains an outcome for. A current
/// (`stado.run-submission.v3`) manifest must list its entries and every
/// retained entry must name its job; one that does not is refused naming
/// the run: read as retaining nothing, the walk was marked done with those
/// jobs unindexed, and every later reader called them never existing. An
/// older manifest, which this walk only reads, keeps the lenient reading.
fn retained_ids<'a>(
    manifest: &'a Map<String, Value>,
    run_id: &str,
) -> Result<Vec<&'a str>, StorageError> {
    let current = manifest.get("schema").and_then(Value::as_str) == Some("stado.run-submission.v3");
    let Some(entries) = manifest.get("entries").and_then(Value::as_array) else {
        if current {
            return Err(StorageError::Other(format!(
                "run {run_id} lists no entries"
            )));
        }
        return Ok(Vec::new());
    };
    let mut retained = Vec::new();
    for entry in entries
        .iter()
        .filter(|entry| entry.get("outcome").is_some())
    {
        match entry.get("job_id").and_then(Value::as_str) {
            Some(job_id) => retained.push(job_id),
            None if current => {
                return Err(StorageError::Other(format!(
                    "run {run_id} retains an outcome that names no job: {entry}"
                )))
            }
            None => {}
        }
    }
    Ok(retained)
}

/// The job a reaped run retained for `job_id`, stamped with the terminal
/// prefix it ended in, or `None` when no run names it. A finished run is
/// reaped: its terminal outcome — the whole job as it ended — stays in the
/// run manifest while the job's own documents and log are deleted, so a job
/// gone for that reason is read back from there, not reported as one that
/// never existed.
///
/// The run is found through the index retention writes (`runs/jobs/<job
/// id>`, [`crate::queue::runs::retained_job_index_path`]): one read of the
/// index and one of that manifest. Jobs retained before the index existed
/// have none, so the first reader that misses walks every run once, indexes
/// every job each one retains and marks the walk done; until then every
/// miss, a job still running included, read every run, and a release that
/// asked after its own fresh jobs read hundreds of manifests each time.
pub async fn retained_job(
    store: &JobStorage,
    job_id: &str,
) -> Result<Option<crate::models::Job>, StorageError> {
    let index = crate::queue::runs::retained_job_index_path(job_id);
    if let Some(run_id) = store.download_text(&index).await? {
        let run_id = run_id.trim();
        return match read_run(store, run_id).await? {
            Some(manifest) => retained_in(&manifest, run_id, job_id),
            None => Ok(None),
        };
    }
    if store.download_text(&walked_path()).await?.is_some() {
        return Ok(None);
    }
    let runs = list_runs(store).await?;
    let mut found = None;
    for run_id in &runs {
        let Some(manifest) = read_run(store, run_id).await? else {
            continue;
        };
        for retained in retained_ids(&manifest, run_id)? {
            crate::queue::runs::index_retained_job(store, retained, run_id).await?;
        }
        if found.is_none() {
            found = retained_in(&manifest, run_id, job_id)?;
        }
    }
    store
        .create_text_if_absent(&walked_path(), &format!("{} runs walked", runs.len()))
        .await?;
    Ok(found)
}

/// Every retained job whose id starts with `prefix` (`job-` and the first
/// hex characters of its id), each stamped with the terminal prefix it ended
/// in: the index retention writes (`runs/jobs/<job id>`) is listed under the
/// prefix and each run it names is read once. A job retained before the
/// index existed is not listed here until a retention pass indexes it;
/// [`retained_job`] finds it by its whole id.
pub async fn retained_jobs_with_prefix(
    store: &JobStorage,
    prefix: &str,
) -> Result<Vec<crate::models::Job>, StorageError> {
    let index_prefix = crate::queue::runs::retained_job_index_path(prefix);
    let index_root = crate::queue::runs::retained_job_index_path("");
    let mut jobs = Vec::new();
    for blob in store.list_blobs_with_meta(&index_prefix).await? {
        let Some(job_id) = blob.name.strip_prefix(&index_root) else {
            continue;
        };
        if let Some(job) = retained_job(store, job_id).await? {
            jobs.push(job);
        }
    }
    Ok(jobs)
}

/// `job_id`'s retained outcome in one run manifest, when it holds one.
fn retained_in(
    manifest: &Map<String, Value>,
    run_id: &str,
    job_id: &str,
) -> Result<Option<crate::models::Job>, StorageError> {
    let Some(outcome) = manifest
        .get("entries")
        .and_then(Value::as_array)
        .and_then(|entries| {
            entries
                .iter()
                .find(|entry| entry.get("job_id").and_then(Value::as_str) == Some(job_id))
        })
        .and_then(|entry| entry.get("outcome"))
    else {
        return Ok(None);
    };
    let (Some(prefix), Some(retained)) = (
        outcome.get("prefix").and_then(Value::as_str),
        outcome.get("job"),
    ) else {
        return Ok(None);
    };
    let mut job = crate::models::Job::from_json(&retained.to_string()).map_err(|error| {
        StorageError::Other(format!(
            "run {run_id} retains an unreadable outcome for {job_id}: {error}"
        ))
    })?;
    job.state = prefix.into();
    Ok(Some(job))
}
