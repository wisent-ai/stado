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
    let mut repaired = 0;
    for candidate in store.list_jobs("queue", 0).await? {
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
                repaired += 1;
                log_fn(&format!(
                    "assigned host-pinned job {} to {pinned_host} instead of {previous:?}",
                    job.job_id
                ));
            }
            Err(StorageError::StorageConflict(_)) | Err(StorageError::NotFound(_)) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(repaired)
}

/// [`assign_jobs`] with an injectable clock + history map, so tests can
/// drive the matcher offline without touching the global TTL caches.
pub async fn assign_jobs_at(
    store: &JobStorage,
    now: DateTime<Utc>,
    history: &History,
    log_fn: &dyn Fn(&str),
) -> Result<usize, StorageError> {
    let mut agents = live_agents(store, now).await?;
    if agents.is_empty() {
        return Ok(0);
    }
    seed_running_jobs(store, &mut agents, now, history, log_fn).await?;
    // Aggregate skip counts + parallel writes:
    // 900+ per-job skip log lines
    // were eating ~300s of the 540s tick budget; serial assignment writes
    // added ~10s. Confirmed live 02:54Z 2026-05-15.
    let mut schedulable: Vec<(i64, f64, Job)> = Vec::new();
    // Insertion-ordered (model, task) -> count, matching the Python dict's
    // stable top-5 ordering (ties keep first-seen order).
    let mut skip_by_key: Vec<((String, String), usize)> = Vec::new();
    let mut to_write: Vec<Job> = Vec::new();
    for mut job in store.list_jobs("queue", 0).await? {
        // Host-pinned jobs route outside the makespan model. The coordinator
        // repairs contradictory derived assignments before routing begins.
        if !job.pinned_host.is_empty() {
            continue;
        }
        let rt = match estimate_runtime(&job.command, job.runtime_seconds_estimate, history) {
            Some(rt) => rt,
            None => {
                if job.priority <= 0 {
                    let mt = extract_model_task(&job.command);
                    if let Some(entry) = skip_by_key.iter_mut().find(|(k, _)| *k == mt) {
                        entry.1 += 1;
                    } else {
                        skip_by_key.push((mt, 1));
                    }
                    // makespan can't optimally ORDER a no-history job, but it
                    // must not leave a stale assigned_to that PINS it to an
                    // agent chosen under a now-obsolete size. gpt-oss-20b was
                    // pinned to the single 96GB local box back when it was
                    // mis-sized 89 (cross-GPU-sum bug); after the per-GPU
                    // sizing fix it fits the idle 80GB fleet, but the skip
                    // path never cleared the pin so it stayed routed to the
                    // saturated box and never dispatched (q frozen ~1h+,
                    // 2026-05-18). Clearing the pin makes it claimable by any
                    // eligible agent (the documented assigned_to="" semantic);
                    // history-backed jobs' ordering is unaffected.
                    if !job.assigned_to.is_empty() {
                        job.assigned_to = String::new();
                        to_write.push(job);
                    }
                    continue;
                }
                // High-priority no-history job (one-off training run, e.g.
                // free_chat_pd GRPO) must not be silently dropped: priority
                // =999999 jobs were starved in queue indefinitely behind the
                // history-backed benchmark backlog (Qwen3 724084db queued
                // 30min+, zero dispatch, 2026-05-15). Conservative long
                // runtime so it still enters schedulable and the priority
                // sort below places it first.
                6.0 * 3600.0
            }
        };
        schedulable.push((-job.priority, -rt, job));
    }
    schedulable.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
    });
    let mut unassigned = 0usize;
    for (_, neg_rt, mut job) in schedulable {
        let vram = job.gpu_mem_gb;
        let chosen = assign_one(&job, &mut agents, -neg_rt, vram);
        match chosen {
            None => {
                unassigned += 1;
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
            .buffered(16)
            .collect::<Vec<Result<(), StorageError>>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;
    }
    let skipped: usize = skip_by_key.iter().map(|(_, n)| n).sum();
    if skipped > 0 {
        let mut top = skip_by_key.clone();
        // Python `sorted(..., key=lambda kv: -kv[1])` — stable, so ties keep
        // first-seen order.
        top.sort_by_key(|entry| std::cmp::Reverse(entry.1));
        let top: Vec<String> = top
            .into_iter()
            .take(5)
            .map(|((m, t), n)| format!("({m},{t}):{n}"))
            .collect();
        log_fn(&format!(
            "makespan: {skipped} skipped; top: {}",
            top.join(", ")
        ));
    }
    if unassigned > 0 {
        log_fn(&format!(
            "makespan: {unassigned} unassigned (no eligible agent)"
        ));
    }
    Ok(to_write.len())
}
