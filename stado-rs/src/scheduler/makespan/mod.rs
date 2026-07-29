//! Makespan-minimizing job-to-agent matcher. Sorts queue by
//! (-priority, -runtime) (LPT in time, runtime from completed-job
//! history keyed by (model, task)), then assigns each job to the
//! eligible agent that finishes it earliest under a VRAM-concurrency
//! model. Writes assigned_to on the queue blob; agent-side enforcement
//! lives in providers/local/helpers/_job_eligible. No runtime guesses:
//! jobs without history AND without an explicit runtime_seconds_estimate
//! stay unassigned and the operator sees a log naming them. The runtime-
//! history machinery lives in [`history`] (split out to keep this module
//! under the 300-line cap in Python; kept split here for parity).
//!
//! Port of `stado/scheduler/makespan/__init__.py`.

pub mod history;

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Utc};
use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;

use crate::catalog::GPU_SIZING;
use crate::models::Job;
use crate::queue::{JobStorage, StorageError};

use history::{extract_model_task, History};

/// Python `HEARTBEAT_TTL_S`.
pub const HEARTBEAT_TTL_S: i64 = 180;

static INSTANCE_HOST_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[^@]+@(.+)$").expect("static regex compiles"));

/// Per-job runtime estimate in seconds. Returns None when neither an
/// explicit estimate nor a matching history entry is available; the
/// caller leaves the job unassigned with a log naming the missing
/// (model, task). A new (model, task) combo must either set
/// runtime_seconds_estimate at submit time or wait for a sibling job
/// to complete and seed history; the matcher refuses to guess.
/// Python `_estimate_runtime` (factored onto primitives so both the
/// queued-job path and the running-blob seed path share it).
fn estimate_runtime(
    command: &str,
    runtime_seconds_estimate: f64,
    history: &History,
) -> Option<f64> {
    if runtime_seconds_estimate > 0.0 {
        return Some(runtime_seconds_estimate);
    }
    let (model, task) = extract_model_task(command);
    if model.is_empty() || task.is_empty() {
        return None;
    }
    history.get(&(model, task)).copied()
}

/// Live-agent projection state. Python `_live_agents` value dict.
#[derive(Debug, Default)]
pub struct AgentInfo {
    pub kind: String,
    pub free_slots: BTreeMap<String, i64>,
    pub total_vram_gb: i64,
    /// (finish_offset_seconds, vram_gb) per projected active slot.
    pub active_slots: Vec<(f64, i64)>,
}

/// Python `_live_agents`: {consumer_id: info} for capacity broadcasts
/// younger than [`HEARTBEAT_TTL_S`].
///
/// Capacity guard: assign_one gates on total_vram_gb only, so a
/// wedged agent (free_vram_gb=0 / free_slots={} but total 80) was a
/// valid target and the 127 gpt-oss-20b jobs dead-pinned to agents
/// that could never claim them (q pinned 348, c frozen 14069,
/// 2026-05-17). Skip an agent with no free VRAM AND no free slots.
async fn live_agents(
    store: &JobStorage,
    now: DateTime<Utc>,
) -> Result<BTreeMap<String, AgentInfo>, StorageError> {
    let paths = store.list_paths("capacity/", 0).await?;
    let texts = download_many(store, &paths).await?;
    let mut agents: BTreeMap<String, AgentInfo> = BTreeMap::new();
    for text in texts.into_iter().flatten() {
        let doc: Value = serde_json::from_str(&text)?;
        let cid = doc
            .get("consumer_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let pub_at = doc
            .get("published_at")
            .and_then(Value::as_str)
            .unwrap_or("");
        // Python lets a malformed published_at crash the tick (no try).
        let published = DateTime::parse_from_rfc3339(pub_at).map_err(|e| {
            StorageError::Other(format!(
                "makespan: capacity blob has malformed published_at {pub_at:?}: {e}"
            ))
        })?;
        let age = (now - published.with_timezone(&Utc)).num_seconds();
        if age > HEARTBEAT_TTL_S {
            continue;
        }
        let free_vram = doc
            .get("free_vram_gb")
            .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
            .unwrap_or(0);
        let free_slots: BTreeMap<String, i64> = doc
            .get("free_slots")
            .and_then(Value::as_object)
            .map(|obj| {
                obj.iter()
                    .map(|(k, v)| (k.clone(), v.as_i64().unwrap_or(0)))
                    .collect()
            })
            .unwrap_or_default();
        if free_vram <= 0 && free_slots.is_empty() {
            continue;
        }
        let kind = doc
            .get("kind")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| cid.split('-').next().unwrap_or("").to_string());
        let total_vram_gb = doc
            .get("total_vram_gb")
            .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
            .unwrap_or(0);
        agents.insert(
            cid,
            AgentInfo {
                kind,
                free_slots,
                total_vram_gb,
                active_slots: vec![],
            },
        );
    }
    Ok(agents)
}

/// For each running/ blob, locate the executing agent (by hostname
/// in instance_ref) and add an active_slot covering its remaining
/// runtime. Without this, a freshly-claimed long job looks invisible
/// and the matcher would pile more work onto an already-loaded agent.
/// Python `_seed_running_jobs`.
async fn seed_running_jobs(
    store: &JobStorage,
    agents: &mut BTreeMap<String, AgentInfo>,
    now: DateTime<Utc>,
    history: &History,
    log_fn: &dyn Fn(&str),
) -> Result<(), StorageError> {
    // consumer_id is "<kind>-<hostname>" (queue/capacity.publish_capacity).
    let mut host_to_cid: HashMap<String, String> = HashMap::new();
    for cid in agents.keys() {
        if let Some((_, host)) = cid.split_once('-') {
            host_to_cid.insert(host.to_string(), cid.clone());
        }
    }
    let paths = store.list_paths("running/", 0).await?;
    let texts = download_many(store, &paths).await?;
    for (path, text) in paths.iter().zip(&texts) {
        let Some(text) = text else { continue }; // moved to completed/failed mid-tick
        let doc: Value = serde_json::from_str(text)?;
        let iref = doc
            .get("instance_ref")
            .and_then(Value::as_str)
            .unwrap_or("");
        let Some(caps) = INSTANCE_HOST_RE.captures(iref) else {
            return Err(StorageError::Other(format!(
                "makespan: running blob {path} has malformed instance_ref {iref:?}; \
                 cannot map to consumer_id"
            )));
        };
        let host = &caps[1];
        let Some(cid) = host_to_cid.get(host) else {
            // Running job points to an agent we no longer track as live.
            // Leave its VRAM out of the projection; the reaper will move
            // the running blob to failed on its own pass. Logging only.
            log_fn(&format!(
                "makespan: running {} on dead host {host}; skipping projection",
                doc.get("job_id").and_then(Value::as_str).unwrap_or("")
            ));
            continue;
        };
        let Some(st) = doc.get("started_at").and_then(Value::as_str) else {
            return Err(StorageError::Other(format!(
                "makespan: running blob {path} has no started_at"
            )));
        };
        let started = DateTime::parse_from_rfc3339(st).map_err(|e| {
            StorageError::Other(format!(
                "makespan: running blob {path} bad started_at {st:?}: {e}"
            ))
        })?;
        let elapsed = (now - started.with_timezone(&Utc)).num_milliseconds() as f64 / 1000.0;
        let command = doc.get("command").and_then(Value::as_str).unwrap_or("");
        let runtime_hint = doc
            .get("runtime_seconds_estimate")
            .and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|i| i as f64)))
            .unwrap_or(0.0);
        let Some(est) = estimate_runtime(command, runtime_hint, history) else {
            // Admin/maintenance commands have no parseable (model, task); the
            // matcher can't predict their finish time. Agent-side smi_free
            // enforces actual VRAM at claim time.
            log_fn(&format!(
                "makespan: skip running {} for seeding",
                doc.get("job_id").and_then(Value::as_str).unwrap_or("")
            ));
            continue;
        };
        let remaining = (est - elapsed.max(0.0)).max(0.0);
        let vram = doc
            .get("gpu_mem_gb")
            .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
            .unwrap_or(0);
        if let Some(info) = agents.get_mut(cid) {
            info.active_slots.push((remaining, vram));
        }
    }
    Ok(())
}

/// Earliest start time (seconds from now) at which new_vram GB
/// becomes free on an agent with the given total VRAM and active slots
/// [(finish_offset_seconds, vram_gb), ...]. Python `_earliest_start`.
fn earliest_start(slots: &[(f64, i64)], new_vram: i64, total_vram: i64) -> f64 {
    let used_now: i64 = slots.iter().map(|(_, v)| v).sum();
    let available_now = total_vram - used_now;
    if available_now >= new_vram {
        return 0.0;
    }
    let mut by_end: Vec<(f64, i64)> = slots.to_vec();
    by_end.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut freed = available_now;
    for (end_time, vram) in &by_end {
        freed += vram;
        if freed >= new_vram {
            return *end_time;
        }
    }
    by_end.last().map(|(end, _)| *end).unwrap_or(0.0)
}

/// Every GCP gpu_type whose required VRAM tier <= `local_vram_gb`.
/// Python `providers.local.helpers._compat_accel_types`, ported here so
/// the matcher can align optimizer-side assignment with agent-side
/// eligibility without pulling in the local-provider module.
fn compat_accel_types(local_vram_gb: i64) -> Vec<&'static str> {
    let mut accels: Vec<&'static str> = Vec::new();
    let Some(sizing) = GPU_SIZING.get(crate::capabilities::ProviderId::Gcp.as_str()) else {
        return accels;
    };
    for (tier, (_, accel)) in sizing {
        if local_vram_gb >= *tier && !accel.is_empty() && !accels.contains(accel) {
            accels.push(accel);
        }
    }
    accels
}

/// Pick the eligible agent that finishes this job earliest; update
/// its active_slots in place. Returns the chosen consumer_id, or None
/// if no agent has enough total VRAM to host the job.
/// Python `_assign_one`.
fn assign_one(
    job: &Job,
    agents: &mut BTreeMap<String, AgentInfo>,
    runtime: f64,
    vram: i64,
) -> Option<String> {
    if job.exclusive {
        return None;
    }
    let mut best_cid: Option<String> = None;
    let mut best_finish: Option<f64> = None;
    for (cid, info) in agents.iter() {
        // Keep optimizer-side assignment aligned with agent-side eligibility.
        // A provider-pinned job assigned to a different consumer kind becomes
        // unclaimable: the pinned agent refuses it, and the assigned agent
        // also refuses it. Confirmed live with a gcp-pinned smoke assigned to
        // local-ubuntu-server.
        if job.pin_to_provider && job.provider != info.kind {
            continue;
        }
        if info.total_vram_gb < vram {
            continue;
        }
        let accel = job.gpu_type.as_str();
        if !accel.is_empty()
            && !info.free_slots.contains_key(accel)
            && !compat_accel_types(info.total_vram_gb).contains(&accel)
        {
            continue;
        }
        let start = earliest_start(&info.active_slots, vram, info.total_vram_gb);
        let finish = start + runtime;
        if best_finish.is_none_or(|best| finish < best) {
            best_finish = Some(finish);
            best_cid = Some(cid.clone());
        }
    }
    let best_cid = best_cid?;
    let best_finish = best_finish?;
    agents
        .get_mut(&best_cid)?
        .active_slots
        .push((best_finish, vram));
    Some(best_cid)
}

/// One pass of makespan-minimizing assignment, reading the clock and the
/// (TTL-cached) runtime history like Python `assign_jobs`. Returns the
/// number of queue blobs whose assigned_to changed this tick.
pub async fn assign_jobs(store: &JobStorage, log_fn: &dyn Fn(&str)) -> Result<usize, StorageError> {
    let history = history::global().history(store, log_fn).await?;
    assign_jobs_at(store, Utc::now(), &history, log_fn).await
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
    // Aggregate skip counts + parallel writes: 900+ per-job skip log lines
    // were eating ~300s of the 540s tick budget; serial assignment writes
    // added ~10s. Confirmed live 02:54Z 2026-05-15.
    let mut schedulable: Vec<(i64, f64, Job)> = Vec::new();
    // Insertion-ordered (model, task) -> count, matching the Python dict's
    // stable top-5 ordering (ties keep first-seen order).
    let mut skip_by_key: Vec<((String, String), usize)> = Vec::new();
    let mut to_write: Vec<Job> = Vec::new();
    for mut job in store.list_jobs("queue", 0).await? {
        // Operator-pinned jobs (pinned_host set at submit) route outside the
        // makespan model: never assign them elsewhere, never clear the pin.
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
        // Fresh read-modify-write of ONLY assigned_to. The Job objects in
        // `to_write` were read at tick start; writing them whole back at
        // tick end resurrects whatever fields changed meanwhile —
        // notably gpu_mem_gb, which an external de-hardcode / the agent's
        // OOM-escalation may have just rewritten. Re-read each blob now
        // and touch only assigned_to so the live gpu_mem_gb is preserved.
        use futures::StreamExt;
        futures::stream::iter(&to_write)
            .map(|job| async move {
                let Some(mut fresh) = store.read_job("queue", &job.job_id).await? else {
                    return Ok::<(), StorageError>(()); // claimed/moved since tick start
                };
                if fresh.assigned_to == job.assigned_to {
                    return Ok(());
                }
                fresh.assigned_to = job.assigned_to.clone();
                store.write_job("queue", &fresh).await
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
        top.sort_by(|a, b| b.1.cmp(&a.1));
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

/// Parallel-download the given blob paths (Python
/// `ThreadPoolExecutor(max_workers=32)` + `pool.map` → `buffered(32)`,
/// path order preserved). A missing blob (TOCTOU between list and
/// download) is None; any other error propagates.
pub(crate) async fn download_many(
    store: &JobStorage,
    paths: &[String],
) -> Result<Vec<Option<String>>, StorageError> {
    use futures::StreamExt;
    futures::stream::iter(paths)
        .map(|path| store.download_text(path))
        .buffered(32)
        .collect::<Vec<Result<Option<String>, StorageError>>>()
        .await
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::local_file::LocalBackend;
    use serde_json::json;
    use std::sync::Arc;

    fn store() -> (tempfile::TempDir, JobStorage) {
        let dir = tempfile::tempdir().unwrap();
        let backend = LocalBackend::new(dir.path().to_str().unwrap()).unwrap();
        (dir, JobStorage::with_backend(Arc::new(backend), "local"))
    }

    const NOW: &str = "2026-05-19T12:00:00+00:00";

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(NOW)
            .unwrap()
            .with_timezone(&Utc)
    }

    async fn publish(store: &JobStorage, cid: &str, kind: &str, total_vram: i64, published: &str) {
        let body = json!({
            "consumer_id": cid,
            "kind": kind,
            "free_slots": {"nvidia-l4": 1},
            "published_at": published,
            "free_vram_gb": total_vram,
            "total_vram_gb": total_vram,
        })
        .to_string();
        store
            .upload_text(&format!("capacity/{cid}.json"), &body)
            .await
            .unwrap();
    }

    fn job(job_id: &str, command: &str, vram: i64) -> Job {
        let mut job = Job::new(job_id, command);
        job.gpu_mem_gb = vram;
        job.created_at = "2026-05-19T00:00:00+00:00".into();
        job
    }

    fn history_with(entries: &[(&str, &str, f64)]) -> History {
        entries
            .iter()
            .map(|(m, t, s)| ((m.to_string(), t.to_string()), *s))
            .collect()
    }

    #[test]
    fn earliest_start_vram_concurrency_model() {
        // Fits right now.
        assert_eq!(earliest_start(&[], 20, 80), 0.0);
        assert_eq!(earliest_start(&[(100.0, 40)], 40, 80), 0.0);
        // Wait for the first slot to free enough VRAM.
        assert_eq!(earliest_start(&[(100.0, 40), (200.0, 40)], 40, 80), 100.0);
        // Needs both slots freed.
        assert_eq!(earliest_start(&[(100.0, 40), (200.0, 40)], 80, 80), 200.0);
        // Never enough: falls back to the last end time.
        assert_eq!(earliest_start(&[(100.0, 40), (200.0, 40)], 120, 80), 200.0);
    }

    #[test]
    fn estimate_runtime_prefers_explicit_over_history() {
        let h = history_with(&[("m", "t", 120.0)]);
        assert_eq!(
            estimate_runtime("x --model m --task t", 42.0, &h),
            Some(42.0)
        );
        assert_eq!(
            estimate_runtime("x --model m --task t", 0.0, &h),
            Some(120.0)
        );
        // No explicit + no matching history -> None (refuses to guess).
        assert_eq!(estimate_runtime("x --model m --task other", 0.0, &h), None);
        assert_eq!(estimate_runtime("admin --restart", 0.0, &h), None);
    }

    #[tokio::test]
    async fn lpt_ordering_and_vram_concurrency_feasibility() {
        let (_dir, store) = store();
        publish(&store, "local-host-a", "local", 80, NOW).await;
        // History: (m,t)=100s. LPT sorts longer runtimes first at equal
        // priority.
        let h = history_with(&[("m", "t", 100.0)]);
        let mut a = job("j-a", "x --model m --task t", 60);
        a.runtime_seconds_estimate = 50.0; // explicit override beats history
        let b = job("j-b", "x --model m --task t", 60); // history 100s
        store.write_job("queue", &a).await.unwrap();
        store.write_job("queue", &b).await.unwrap();

        let written = assign_jobs_at(&store, now(), &h, &|_| ()).await.unwrap();
        assert_eq!(written, 2);
        // j-b (100s) was considered first: start 0 on the 80GB agent, taking
        // 60 of 80 GB. j-a (50s) does not fit alongside (60+60 > 80) and
        // starts when j-b finishes — same agent, still earliest finish.
        let a_back = store.read_job("queue", "j-a").await.unwrap().unwrap();
        let b_back = store.read_job("queue", "j-b").await.unwrap().unwrap();
        assert_eq!(b_back.assigned_to, "local-host-a");
        assert_eq!(a_back.assigned_to, "local-host-a");

        // A job needing more VRAM than every agent is unassigned (and a
        // stale assigned_to is cleared).
        let mut big = job("j-big", "x --model m --task t", 160);
        big.assigned_to = "local-host-a".into();
        store.write_job("queue", &big).await.unwrap();
        let logs: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
        let written = assign_jobs_at(&store, now(), &h, &|m| logs.lock().unwrap().push(m.into()))
            .await
            .unwrap();
        assert!(store
            .read_job("queue", "j-big")
            .await
            .unwrap()
            .unwrap()
            .assigned_to
            .is_empty());
        assert!(written >= 1);
        assert!(logs
            .lock()
            .unwrap()
            .iter()
            .any(|l| l.contains("1 unassigned")));
    }

    #[tokio::test]
    async fn makespan_picks_earliest_finishing_agent() {
        let (_dir, store) = store();
        publish(&store, "local-idle", "local", 80, NOW).await;
        publish(&store, "local-busy", "local", 80, NOW).await;
        // local-busy is running a 60GB job with ~600s remaining.
        let running = json!({
            "job_id": "r1",
            "command": "x --model m --task t",
            "state": "running",
            "instance_ref": "agent@busy",
            "started_at": NOW,
            "gpu_mem_gb": 60,
            "runtime_seconds_estimate": 600,
        })
        .to_string();
        store
            .upload_text("running/r1.json", &running)
            .await
            .unwrap();

        let h = history_with(&[("m", "t", 100.0)]);
        let candidate = job("j-c", "x --model m --task t", 40);
        store.write_job("queue", &candidate).await.unwrap();
        let written = assign_jobs_at(&store, now(), &h, &|_| ()).await.unwrap();
        assert_eq!(written, 1);
        // The idle agent finishes at +100s; local-busy at +700s.
        assert_eq!(
            store
                .read_job("queue", "j-c")
                .await
                .unwrap()
                .unwrap()
                .assigned_to,
            "local-idle"
        );
    }

    #[tokio::test]
    async fn pinned_host_jobs_are_never_touched() {
        let (_dir, store) = store();
        publish(&store, "local-host-a", "local", 80, NOW).await;
        let mut pinned = job("j-pin", "x --model m --task t", 40);
        pinned.pinned_host = "local-other-box".into();
        store.write_job("queue", &pinned).await.unwrap();

        let h = history_with(&[("m", "t", 100.0)]);
        let written = assign_jobs_at(&store, now(), &h, &|_| ()).await.unwrap();
        assert_eq!(written, 0);
        let back = store.read_job("queue", "j-pin").await.unwrap().unwrap();
        assert_eq!(back.pinned_host, "local-other-box");
        assert!(back.assigned_to.is_empty());
    }

    #[tokio::test]
    async fn no_history_low_priority_is_skipped_and_unpinned() {
        let (_dir, store) = store();
        publish(&store, "local-host-a", "local", 80, NOW).await;
        // No history entry and no explicit estimate, priority 0, with a
        // stale pin from an obsolete sizing pass.
        let mut stale = job("j-stale", "x --model m-new --task t-new", 40);
        stale.assigned_to = "local-host-a".into();
        store.write_job("queue", &stale).await.unwrap();

        let logs: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
        let written = assign_jobs_at(&store, now(), &History::new(), &|m| {
            logs.lock().unwrap().push(m.into())
        })
        .await
        .unwrap();
        assert_eq!(written, 1);
        // Stale pin cleared; job left for any eligible agent, skip logged.
        assert!(store
            .read_job("queue", "j-stale")
            .await
            .unwrap()
            .unwrap()
            .assigned_to
            .is_empty());
        let logs = logs.lock().unwrap();
        assert!(
            logs.iter()
                .any(|l| l.contains("1 skipped; top: (m-new,t-new):1")),
            "{logs:?}"
        );
    }

    #[tokio::test]
    async fn high_priority_no_history_gets_conservative_runtime() {
        let (_dir, store) = store();
        publish(&store, "local-host-a", "local", 80, NOW).await;
        let mut prio = job("j-prio", "x --model m-new --task t-new", 40);
        prio.priority = 999999;
        store.write_job("queue", &prio).await.unwrap();

        let written = assign_jobs_at(&store, now(), &History::new(), &|_| ())
            .await
            .unwrap();
        assert_eq!(written, 1);
        assert_eq!(
            store
                .read_job("queue", "j-prio")
                .await
                .unwrap()
                .unwrap()
                .assigned_to,
            "local-host-a"
        );
    }

    #[tokio::test]
    async fn exclusive_and_provider_pinned_jobs_are_not_assigned() {
        let (_dir, store) = store();
        publish(&store, "local-host-a", "local", 80, NOW).await;
        let mut excl = job("j-excl", "x --model m --task t", 40);
        excl.exclusive = true;
        store.write_job("queue", &excl).await.unwrap();
        let mut pinned_provider = job("j-prov", "x --model m --task t", 40);
        pinned_provider.pin_to_provider = true;
        pinned_provider.provider = "gcp".into();
        store.write_job("queue", &pinned_provider).await.unwrap();

        let h = history_with(&[("m", "t", 100.0)]);
        let logs: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
        assign_jobs_at(&store, now(), &h, &|m| logs.lock().unwrap().push(m.into()))
            .await
            .unwrap();
        assert!(store
            .read_job("queue", "j-excl")
            .await
            .unwrap()
            .unwrap()
            .assigned_to
            .is_empty());
        assert!(store
            .read_job("queue", "j-prov")
            .await
            .unwrap()
            .unwrap()
            .assigned_to
            .is_empty());
        assert!(logs
            .lock()
            .unwrap()
            .iter()
            .any(|l| l.contains("2 unassigned")));
    }

    #[tokio::test]
    async fn stale_capacity_and_wedged_agents_are_not_targets() {
        let (_dir, store) = store();
        let stale = (now() - chrono::Duration::seconds(HEARTBEAT_TTL_S + 10)).to_rfc3339();
        publish(&store, "local-dead", "local", 80, &stale).await;
        // Wedged: free_vram 0 and no free slots -> skipped as a target.
        let wedged = json!({
            "consumer_id": "local-wedged", "kind": "local",
            "free_slots": {}, "free_vram_gb": 0, "total_vram_gb": 80,
            "published_at": NOW,
        })
        .to_string();
        store
            .upload_text("capacity/local-wedged.json", &wedged)
            .await
            .unwrap();

        let h = history_with(&[("m", "t", 100.0)]);
        let candidate = job("j-c", "x --model m --task t", 40);
        store.write_job("queue", &candidate).await.unwrap();
        // No live agents at all -> 0 writes, job untouched.
        let written = assign_jobs_at(&store, now(), &h, &|_| ()).await.unwrap();
        assert_eq!(written, 0);
        assert!(store
            .read_job("queue", "j-c")
            .await
            .unwrap()
            .unwrap()
            .assigned_to
            .is_empty());
    }

    #[tokio::test]
    async fn running_seed_on_dead_host_is_logged_not_fatal() {
        let (_dir, store) = store();
        publish(&store, "local-host-a", "local", 80, NOW).await;
        let running = json!({
            "job_id": "r-ghost", "command": "x --model m --task t",
            "instance_ref": "agent@vanished-host", "started_at": NOW, "gpu_mem_gb": 40,
        })
        .to_string();
        store
            .upload_text("running/r-ghost.json", &running)
            .await
            .unwrap();
        let h = history_with(&[("m", "t", 100.0)]);
        let logs: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
        assign_jobs_at(&store, now(), &h, &|m| logs.lock().unwrap().push(m.into()))
            .await
            .unwrap();
        assert!(logs
            .lock()
            .unwrap()
            .iter()
            .any(|l| l.contains("dead host vanished-host")));
    }

    #[tokio::test]
    async fn assignment_rewrite_preserves_live_gpu_mem_gb() {
        let (_dir, store) = store();
        publish(&store, "local-host-a", "local", 80, NOW).await;
        let candidate = job("j-c", "x --model m --task t", 40);
        store.write_job("queue", &candidate).await.unwrap();
        // Simulate OOM-escalation racing the tick: the blob's gpu_mem_gb
        // changed after the matcher's tick-start read.
        let mut live = store.read_job("queue", "j-c").await.unwrap().unwrap();
        live.gpu_mem_gb = 71;
        store.write_job("queue", &live).await.unwrap();

        let h = history_with(&[("m", "t", 100.0)]);
        let written = assign_jobs_at(&store, now(), &h, &|_| ()).await.unwrap();
        assert_eq!(written, 1);
        let back = store.read_job("queue", "j-c").await.unwrap().unwrap();
        assert_eq!(back.assigned_to, "local-host-a");
        assert_eq!(
            back.gpu_mem_gb, 71,
            "assigned_to rewrite must not resurrect the stale size"
        );
    }
}
