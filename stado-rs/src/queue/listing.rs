//! Priority-marker index (`queue_priority/<inv_prio>-<ts>-<jid>.json`
//! markers so name-ascending sort = priority-desc + FIFO), the `list_jobs`
//! bulk fetch, the top-N priority listing, the priority-first listing, and
//! the metadata-prefiltered fitting-jobs listing.
//!
//! Port of `stado/queue/listing/__init__.py`.
//!
//! Known Python bug (ported as intended): `listing/__init__.py:120`,
//! `capacity.py:121` and `leases/__init__.py:143` reference
//! `store._azure_backend`, an attribute that never exists (the backend
//! handle is `_blob_backend`). The intended behavior is a single backend
//! handle — exactly what `JobStorage` carries here — so `list_fitting`
//! always takes the metadata-prefiltering path.

use std::collections::HashSet;

use futures::StreamExt;

use crate::models::Job;

use super::storage::JobStorage;
use super::{json_str, migrations, StorageError};

/// Sortable name component: lower = higher real priority + older.
///
/// Python `priority_key`: priority is clamped to 0..=99999999, inverted,
/// and zero-padded to 8 digits, followed by the ISO created_at.
pub fn priority_key(job: &Job) -> String {
    let prio = job.priority.clamp(0, 99_999_999);
    let inv = 99_999_999 - prio;
    format!("{inv:08}-{}", job.created_at)
}

/// Index entry for priority>0 jobs.
pub async fn write_marker(store: &JobStorage, job: &Job) -> Result<(), StorageError> {
    let name = format!("queue_priority/{}-{}.json", priority_key(job), job.job_id);
    // Python `json.dumps({"job_id": ..., "priority": int(...)})` with
    // default separators.
    let body = format!("{{\"job_id\": {}, \"priority\": {}}}", json_str(&job.job_id), job.priority);
    store.upload_text(&name, &body).await
}

/// Remove any priority marker(s) for this job_id.
pub async fn delete_marker(store: &JobStorage, job_id: &str) -> Result<(), StorageError> {
    let suffix = format!("-{job_id}.json");
    for path in store.list_paths("queue_priority/", 0).await? {
        if path.ends_with(&suffix) {
            store.delete_blob(&path).await?;
        }
    }
    Ok(())
}

/// Parallel-fetch job JSONs under `{prefix}/`.
///
/// Python `list_jobs`: `oldest_first > 0` caps to that many blobs picked by
/// creation time — required for queue/ where 14k+ blobs would otherwise
/// force the scheduler to download every JSON before slicing per_tick_cap
/// and blow the 60s function timeout. Fetches fan out 10 ways (Python
/// `ThreadPoolExecutor(max_workers=min(10, len(paths)))` →
/// `buffer_unordered(10)`); result order follows the path listing, as in
/// Python's `pool.map`.
pub async fn list_jobs(
    store: &JobStorage,
    prefix: &str,
    oldest_first: usize,
) -> Result<Vec<Job>, StorageError> {
    let paths: Vec<String> = store
        .list_paths(&format!("{prefix}/"), oldest_first)
        .await?
        .into_iter()
        .filter(|p| p.ends_with(".json"))
        .collect();
    if paths.is_empty() {
        return Ok(vec![]);
    }
    let texts: Vec<Option<String>> = futures::stream::iter(&paths)
        .map(|path| store.download_text(path))
        .buffered(10)
        .collect::<Vec<Result<Option<String>, StorageError>>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;
    let mut jobs = Vec::new();
    for data in texts.into_iter().flatten() {
        jobs.push(Job::from_json(&data)?);
    }
    Ok(jobs)
}

/// Python `_download_or_none` fanned out over `paths` with `workers`
/// concurrent fetches. `ThreadPoolExecutor(max_workers=...)` + `pool.map`
/// becomes `buffered(workers)`, which preserves the path order in the
/// output, as `pool.map` does.
async fn download_many_or_none(
    store: &JobStorage,
    paths: &[String],
    workers: usize,
) -> Vec<Option<String>> {
    futures::stream::iter(paths)
        .map(|path| async move { store.download_text(path).await.ok().flatten() })
        .buffered(workers.max(1))
        .collect()
        .await
}

/// Fetch the top_n highest-priority jobs from `prefix/` via the
/// `queue_priority/` index. Markers sort ascending by name =
/// (inv_priority, created_at), so the first `top_n` give priority-desc +
/// FIFO.
///
/// Stale priority markers are expected: a job can move out of queue/ after
/// its marker was written, and older versions only deleted markers on the
/// queue -> running path. Do not let stale top markers consume the whole
/// top_n budget, or high-priority fresh jobs disappear behind dead markers.
/// Keep the scan bounded because agents call this in their polling loop.
pub async fn list_top_n(
    store: &JobStorage,
    prefix: &str,
    top_n: usize,
) -> Result<Vec<Job>, StorageError> {
    if top_n == 0 {
        return Ok(vec![]);
    }
    let marker_paths: Vec<String> = store
        .list_paths("queue_priority/", 0)
        .await?
        .into_iter()
        .filter(|p| p.ends_with(".json"))
        .collect();
    if marker_paths.is_empty() {
        return Ok(vec![]);
    }

    let mut out: Vec<Job> = Vec::new();
    let chunk = 50.max(top_n);
    let max_scan = marker_paths.len().min((top_n * 20).max(top_n));
    let mut i = 0;
    while i < max_scan {
        let paths = &marker_paths[i..(i + chunk).min(max_scan)];
        let bodies = download_many_or_none(store, paths, 10.min(paths.len())).await;
        let mut job_ids: Vec<(&str, String)> = Vec::new();
        for (path, body) in paths.iter().zip(&bodies) {
            let Some(body) = body else { continue };
            // Strict-raise on corrupt marker JSON (post-extraction Python
            // parity); a missing/non-string job_id just skips the marker.
            let value: serde_json::Value = serde_json::from_str(body)?;
            if let Some(jid) = value.get("job_id").and_then(serde_json::Value::as_str) {
                job_ids.push((path, jid.to_string()));
            }
        }
        if job_ids.is_empty() {
            i += chunk;
            continue;
        }

        let job_paths: Vec<String> =
            job_ids.iter().map(|(_, jid)| format!("{prefix}/{jid}.json")).collect();
        let blobs = download_many_or_none(store, &job_paths, 10.min(job_paths.len())).await;
        for ((_marker_path, _jid), data) in job_ids.iter().zip(blobs) {
            if let Some(data) = data {
                out.push(Job::from_json(&data)?);
                if out.len() >= top_n {
                    return Ok(out);
                }
            }
        }
        i += chunk;
    }
    Ok(out)
}

/// Priority markers first, then FIFO oldest_first. Deduped by job_id.
/// Calls [`migrations::backfill_priority_markers`] up-front so any
/// pre-0.4.26 queued jobs get retroactive markers.
pub async fn list_priority_first(
    store: &JobStorage,
    prefix: &str,
    cap: usize,
) -> Result<Vec<Job>, StorageError> {
    migrations::backfill_priority_markers(store, migrations::BACKFILL_BATCH).await?;
    let pri = list_top_n(store, prefix, cap).await?;
    let fifo = list_jobs(store, prefix, cap).await?;
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for job in pri.into_iter().chain(fifo) {
        if seen.insert(job.job_id.clone()) {
            out.push(job);
        }
    }
    Ok(out)
}

/// Priority-aware fitting jobs: `queue_priority/` markers first, then FIFO
/// from `queue/`. Metadata stamping filters non-fitting blobs before
/// download.
///
/// Python has a gsutil-only fallback (`store._azure_backend is None and
/// store._sdk_bucket is None` — the first half of which is the never-true
/// `_azure_backend` bug) that downloads everything. The intended behavior
/// is the metadata-prefiltering path, and every Rust `BlobBackend`
/// implements `list_blobs_with_meta`, so only that path is ported.
pub async fn list_fitting(
    store: &JobStorage,
    prefix: &str,
    max_gpu_mem_gb: i64,
    cap: usize,
) -> Result<Vec<Job>, StorageError> {
    let mut out: Vec<Job> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    if prefix == "queue" {
        for job in list_top_n(store, prefix, cap).await? {
            if job.gpu_mem_gb <= max_gpu_mem_gb && !seen.contains(&job.job_id) {
                seen.insert(job.job_id.clone());
                out.push(job);
            }
        }
    }
    let mut eligible_paths: Vec<String> = Vec::new();
    for blob in store.list_blobs_with_meta(&format!("{prefix}/")).await? {
        let name = blob.name.rsplit('/').next().unwrap_or("");
        let jid = name.strip_suffix(".json").unwrap_or(name);
        if !blob.name.ends_with(".json") || seen.contains(jid) {
            continue;
        }
        // mem_str is None on blobs that predate the metadata stamp -> treat
        // as eligible. A corrupt int now raises so the operator sees that
        // the metadata is misbehaving (Python `int(mem_str)` ValueError).
        match blob.metadata.get("gpu_mem_gb") {
            None => eligible_paths.push(blob.name.clone()),
            Some(mem_str) => {
                let mem: i64 = mem_str.parse().map_err(|_| {
                    StorageError::Other(format!(
                        "corrupt gpu_mem_gb metadata on {}: {mem_str:?}",
                        blob.name
                    ))
                })?;
                if mem <= max_gpu_mem_gb {
                    eligible_paths.push(blob.name.clone());
                }
            }
        }
        if eligible_paths.len() + out.len() >= cap {
            break;
        }
    }

    // Python `_read`: download errors propagate; only a missing blob (None)
    // is skipped.
    let texts: Vec<Option<String>> = futures::stream::iter(&eligible_paths)
        .map(|path| store.download_text(path))
        .buffered(32)
        .collect::<Vec<Result<Option<String>, StorageError>>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;
    for data in texts.into_iter().flatten() {
        let job = Job::from_json(&data)?;
        if job.gpu_mem_gb <= max_gpu_mem_gb {
            out.push(job);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::local_file::LocalBackend;
    use std::sync::Arc;

    #[test]
    fn priority_key_matches_python_format() {
        let mut job = Job::new("j", "echo");
        job.priority = 5;
        job.created_at = "2026-01-02T03:04:05+00:00".into();
        assert_eq!(priority_key(&job), "99999994-2026-01-02T03:04:05+00:00");

        // Clamped: negative -> 0, huge -> 99999999.
        job.priority = -3;
        assert_eq!(priority_key(&job), "99999999-2026-01-02T03:04:05+00:00");
        job.priority = 1_000_000_000;
        assert_eq!(priority_key(&job), "00000000-2026-01-02T03:04:05+00:00");
    }

    fn store() -> (tempfile::TempDir, JobStorage) {
        let dir = tempfile::tempdir().unwrap();
        let backend = LocalBackend::new(dir.path().to_str().unwrap()).unwrap();
        (dir, JobStorage::with_backend(Arc::new(backend), "local"))
    }

    fn job(job_id: &str, priority: i64, gpu_mem_gb: i64, created_at: &str) -> Job {
        let mut job = Job::new(job_id, "echo hi");
        job.priority = priority;
        job.gpu_mem_gb = gpu_mem_gb;
        job.created_at = created_at.into();
        job
    }

    fn ids(jobs: &[Job]) -> Vec<&str> {
        jobs.iter().map(|j| j.job_id.as_str()).collect()
    }

    #[tokio::test]
    async fn list_top_n_is_priority_desc_then_fifo_and_skips_stale_markers() {
        let (_dir, store) = store();
        // Same priority -> FIFO by created_at; higher priority first.
        store.write_job("queue", &job("p10-old", 10, 8, "2026-01-01T00:00:00+00:00")).await.unwrap();
        store.write_job("queue", &job("p10-new", 10, 8, "2026-01-02T00:00:00+00:00")).await.unwrap();
        store.write_job("queue", &job("p5", 5, 8, "2025-12-31T00:00:00+00:00")).await.unwrap();
        store.write_job("queue", &job("p0", 0, 8, "2025-01-01T00:00:00+00:00")).await.unwrap();
        // Stale marker: outranks everything but the queue blob is gone.
        store.write_job("queue", &job("ghost", 100, 8, "2024-01-01T00:00:00+00:00")).await.unwrap();
        store.delete_blob("queue/ghost.json").await.unwrap();

        let top = list_top_n(&store, "queue", 3).await.unwrap();
        assert_eq!(ids(&top), vec!["p10-old", "p10-new", "p5"]);

        // top_n larger than the marker set is fine; 0 short-circuits.
        assert!(list_top_n(&store, "queue", 0).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn list_priority_first_puts_markers_ahead_of_fifo_and_dedupes() {
        let (_dir, store) = store();
        // FIFO order comes from blob creation time (local backend ctime).
        store.write_job("queue", &job("fifo-a", 0, 8, "2026-01-01T00:00:00+00:00")).await.unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        store.write_job("queue", &job("fifo-b", 0, 8, "2026-01-02T00:00:00+00:00")).await.unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        store.write_job("queue", &job("prio", 5, 8, "2026-01-03T00:00:00+00:00")).await.unwrap();

        let jobs = list_priority_first(&store, "queue", 10).await.unwrap();
        assert_eq!(ids(&jobs), vec!["prio", "fifo-a", "fifo-b"]);
    }

    #[tokio::test]
    async fn list_fitting_filters_on_vram_with_metadata_prefilter() {
        let (_dir, store) = store();
        store.write_job("queue", &job("fit-lo", 0, 8, "2026-01-01T00:00:00+00:00")).await.unwrap();
        store.write_job("queue", &job("fit-prio", 5, 24, "2026-01-02T00:00:00+00:00")).await.unwrap();
        store.write_job("queue", &job("too-big", 9, 80, "2026-01-03T00:00:00+00:00")).await.unwrap();
        // Pre-metadata-stamp blob (uploaded raw): treated as eligible, then
        // filtered on the downloaded body (16 <= 24 fits).
        let raw = job("no-meta", 0, 16, "2026-01-04T00:00:00+00:00");
        store.upload_text("queue/no-meta.json", &raw.to_json()).await.unwrap();

        let fitting = list_fitting(&store, "queue", 24, 4000).await.unwrap();
        let mut got = ids(&fitting);
        got.sort_unstable();
        assert_eq!(got, vec!["fit-lo", "fit-prio", "no-meta"]);
        // The priority job comes through the marker pass exactly once.
        assert_eq!(fitting.iter().filter(|j| j.job_id == "fit-prio").count(), 1);

        // Nothing fits a 4 GB card.
        assert!(list_fitting(&store, "queue", 4, 4000).await.unwrap().is_empty());
    }
}
