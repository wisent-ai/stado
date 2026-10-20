//! The assignment tick itself: order the queue by (-priority, -runtime),
//! walk it through the placement decision, aggregate the skip log and write
//! the resulting assigned_to values back with compare-and-swap. Split out of
//! `makespan/mod.rs`.

use chrono::{DateTime, Utc};

use crate::models::Job;
use crate::queue::{JobStorage, StorageError};

use super::agents::{estimate_runtime, live_agents, seed_running_jobs};
use super::history::{self, extract_model_task, History};
use super::matcher::assign_one;

/// One pass of makespan-minimizing assignment, reading the clock and the
/// (TTL-cached) runtime history like Python `assign_jobs`. Returns the
/// number of queue blobs whose assigned_to changed this tick.
pub async fn assign_jobs(store: &JobStorage, log_fn: &dyn Fn(&str)) -> Result<usize, StorageError> {
    let history = history::global().history(store, log_fn).await?;
    assign_jobs_at(store, Utc::now(), &history, log_fn).await
}
/// Make every operator host pin explicit in the derived assignment field.
///
/// An empty assignment normally means any eligible agent may race to claim.
/// Mirroring `pinned_host` into `assigned_to` keeps older coordinators from
/// deriving a contradictory consumer while preserving the agent's hard-pin
/// check. Versioned writes prevent a coordinator tick from resurrecting a job
/// that an agent moved out of the queue concurrently.
pub async fn repair_conflicting_pinned_assignments(
    store: &JobStorage,
    log_fn: &dyn Fn(&str),
) -> Result<usize, StorageError> {
    let mut repaired: Vec<String> = Vec::new();
    for candidate in store.list_jobs("queue").await? {
        if candidate.pinned_host.is_empty()
            || candidate
                .assigned_to
                .eq_ignore_ascii_case(&candidate.pinned_host)
        {
            continue;
        }
        let path = format!("queue/{}.json", candidate.job_id);
        let Some(versioned) = store.read_text_versioned(&path).await? else {
            continue;
        };
        let mut job = Job::from_json(&versioned.content)?;
        if job.state != crate::models::job_state::QUEUED
            || job.pinned_host.is_empty()
            || job.assigned_to.eq_ignore_ascii_case(&job.pinned_host)
        {
            continue;
        }
        let pinned_host = job.pinned_host.clone();
        let previous = std::mem::replace(&mut job.assigned_to, pinned_host.clone());
        match store
            .compare_and_swap_text(&path, &versioned.version, &job.to_json())
            .await
        {
            Ok(_) => {
                log_fn(&format!(
                    "assigned host-pinned job {} to {pinned_host} instead of {previous:?}",
                    job.job_id
                ));
                repaired.push(job.job_id);
            }
            Err(StorageError::StorageConflict(_)) | Err(StorageError::NotFound(_)) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(repaired.len())
}

/// [`assign_jobs`] with an injectable clock + history map, so tests can
/// drive the matcher offline without touching the global TTL caches.
pub async fn assign_jobs_at(
    store: &JobStorage,
    now: DateTime<Utc>,
    history: &History,
    log_fn: &dyn Fn(&str),
) -> Result<usize, StorageError> {
    let mut to_write: Vec<Job> = Vec::new();
    let mut agents = live_agents(store, now).await?;
    if agents.is_empty() {
        return Ok(to_write.len());
    }
    seed_running_jobs(store, &mut agents, now, history, log_fn).await?;
    // Aggregate skip counts + parallel writes: hundreds of per-job skip log
    // lines eat most of a tick's budget, and serial assignment writes add
    // seconds more.
    let mut schedulable: Vec<(i64, f64, Job)> = Vec::new();
    // Insertion-ordered (model, task) -> the jobs skipped for it; ties keep
    // first-seen order.
    let mut skip_by_key: Vec<((String, String), Vec<String>)> = Vec::new();
    for mut job in store.list_jobs("queue").await? {
        // Host-pinned jobs route outside the makespan model. The coordinator
        // repairs contradictory derived assignments before routing begins.
        if !job.pinned_host.is_empty() {
            continue;
        }
        let rt = match estimate_runtime(&job.command, job.runtime_seconds_estimate, history) {
            Some(rt) => rt,
            None => {
                let mt = extract_model_task(&job.command);
                match skip_by_key.iter_mut().find(|(k, _)| *k == mt) {
                    Some((_, skipped)) => skipped.push(job.job_id.clone()),
                    None => skip_by_key.push((mt, vec![job.job_id.clone()])),
                }
                // Priority does not supply a runtime. Clear any obsolete
                // derived assignment so eligible workers can claim this job
                // without inventing a finish time for the makespan model.
                if !job.assigned_to.is_empty() {
                    job.assigned_to = String::new();
                    to_write.push(job);
                }
                continue;
            }
        };
        schedulable.push((-job.priority, -rt, job));
    }
    schedulable.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.total_cmp(&b.1)));
    let mut unassigned: Vec<String> = Vec::new();
    for (_, neg_rt, mut job) in schedulable {
        let vram = job.gpu_mem_gb;
        let chosen = assign_one(&job, &mut agents, -neg_rt, vram);
        match chosen {
            None => {
                unassigned.push(job.job_id.clone());
                if !job.assigned_to.is_empty() {
                    job.assigned_to = String::new();
                    to_write.push(job);
                }
            }
            Some(cid) => {
                if job.assigned_to == cid {
                    continue;
                }
                job.assigned_to = cid;
                to_write.push(job);
            }
        }
    }
    if !to_write.is_empty() {
        // CAS-update only assigned_to on the still-current queued generation;
        // a stale tick cannot recreate a job already claimed or terminated.
        use futures::StreamExt;
        futures::stream::iter(&to_write)
            .map(|job| async move {
                store
                    .update_queued_assignment(&job.job_id, &job.assigned_to)
                    .await?;
                Ok::<(), StorageError>(())
            })
            .buffered(crate::queue::migrations::bulk_workers()?)
            .collect::<Vec<Result<(), StorageError>>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;
    }
    if !skip_by_key.is_empty() {
        let skipped: usize = skip_by_key.iter().map(|(_, jobs)| jobs.len()).sum();
        // Preserve first-seen order for groups with the same count.
        skip_by_key.sort_by_key(|entry| std::cmp::Reverse(entry.1.len()));
        let groups: Vec<String> = skip_by_key
            .into_iter()
            .map(|((m, t), jobs)| format!("({m},{t}):{}", jobs.len()))
            .collect();
        log_fn(&format!(
            "makespan: {skipped} skipped; by model/task: {}",
            groups.join(", ")
        ));
    }
    if !unassigned.is_empty() {
        log_fn(&format!(
            "makespan: {} unassigned (no eligible agent)",
            unassigned.len()
        ));
    }
    Ok(to_write.len())
}
