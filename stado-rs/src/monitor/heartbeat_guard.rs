//! Per-job heartbeat freshness check used by the reaper to avoid
//! destroying productive VMs.
//!
//! Port of `stado/monitor/heartbeat_guard.py`.
//!
//! The reaper's primary signal is the agent's capacity broadcast in
//! gs://<bucket>/capacity/<consumer_id>.json. When the agent runs a
//! long training subprocess the broadcast loop can starve past
//! CAPACITY_STALE_SECONDS even though the agent process is alive and
//! the training is actively producing checkpoints. Reaping that VM
//! destroys hours of work and forces the job to restart from the last
//! checkpoint (or step 0 if no checkpoints exist).
//!
//! This module provides a second signal — the per-job heartbeat at
//! gs://<bucket>/status/<job_id>/heartbeat — that is written by the
//! running job itself (via the agent's status-watchdog cron) and is
//! NOT coupled to the agent's broadcast loop. If ANY job assigned to
//! a VM has a fresh heartbeat, the agent is alive and the reap is
//! deferred.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use chrono::{DateTime, Utc};
use regex::Regex;

use crate::models::{isoformat_utc, job_state, Job};
use crate::queue::{JobStorage, StorageError};

/// Sentinel returned by [`fresh_jids_pointing_to_ref`] when the running/
/// listing itself fails, so callers defer (treat the VM as in-use).
pub const LIST_FAILED_SENTINEL: &str = "__list_failed__";

static TS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}(?:\.\d+)?Z?)")
        .expect("static regex compiles")
});

static CKPT_URI_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"--checkpoint-gcs-uri\s+(\S+)").expect("static regex compiles"));

/// Current time as unix seconds (float, microsecond precision) — the
/// Python `time.time()` slot.
fn now_unix() -> f64 {
    unix_seconds(Utc::now())
}

/// `datetime.timestamp()` equivalent: seconds + microseconds fraction.
fn unix_seconds(dt: DateTime<Utc>) -> f64 {
    dt.timestamp() as f64 + f64::from(dt.timestamp_subsec_micros()) / 1e6
}

/// Lenient ISO-8601 → UTC parse used for job.started_at / diag timestamps.
/// Job timestamps are Strings produced by Python `datetime.isoformat()` /
/// Rust `Utc::to_rfc3339()`; parse RFC3339 first, then fall back to a naive
/// `YYYY-MM-DD[T ]HH:MM:SS[.f]` read assumed UTC (Python `fromisoformat`
/// accepts both separators, and every writer here stamps UTC, so
/// assume-UTC matches production data). Returns None on unparseable input,
/// which callers treat per the Python except-branches they port.
pub(crate) fn parse_iso_lenient(s: &str) -> Option<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc));
    }
    for fmt in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f"] {
        if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            return Some(naive.and_utc());
        }
    }
    None
}

/// Parse an ISO-8601 timestamp from the heartbeat blob and return
/// unix seconds. The agent writes lines like:
///     RUNNING 2026-05-13T00:26:33.130155+00:00
/// Returns None if no parseable timestamp is found.
fn parse_heartbeat_ts(text: &str) -> Option<f64> {
    if text.is_empty() {
        return None;
    }
    let caps = TS_RE.captures(text)?;
    let mut raw = caps[1].replace(' ', "T");
    // Python rstrip("Z").
    while raw.ends_with('Z') {
        raw.pop();
    }
    if raw.contains('.') {
        let dot = raw.find('.').expect("contains checked");
        let head = &raw[..dot];
        let frac = &raw[dot + 1..];
        // First +/- in the fraction starts the timezone.
        let (frac_digits, tz) = match frac.find(['+', '-']) {
            Some(i) => (&frac[..i], &frac[i..]),
            None => (frac, ""),
        };
        let frac_digits: String = frac_digits.chars().take(6).collect();
        raw = format!("{head}.{frac_digits}{tz}");
    }
    if !raw.contains('+') && !raw.ends_with('Z') {
        raw.push_str("+00:00");
    }
    // Python `datetime.fromisoformat(raw)`; after the normalization above
    // the string is always RFC3339-shaped. A chrono rejection maps to None
    // (Python would raise ValueError, but only on inputs the regex +
    // normalization can produce for negative-offset zones, which production
    // never writes — heartbeats are always UTC).
    let dt = DateTime::parse_from_rfc3339(&raw).ok()?;
    Some(unix_seconds(dt.with_timezone(&Utc)))
}

/// True iff ANY job in jids has a heartbeat blob whose embedded
/// timestamp is younger than threshold_seconds. Used by the reaper:
/// if the agent's capacity blob is stale but a job assigned to its
/// VM is still heartbeating, the agent is alive — busy in the
/// training subprocess — and the VM should NOT be deleted.
pub async fn any_job_heartbeat_fresh(
    store: &JobStorage,
    jids: &[String],
    threshold_seconds: f64,
) -> bool {
    let now = now_unix();
    for jid in jids {
        if jid.is_empty() {
            continue;
        }
        let text = match store
            .download_text(&format!("status/{jid}/heartbeat"))
            .await
        {
            Ok(text) => text,
            Err(_) => {
                // A coordinator-side GCS read failure is NOT proof the job
                // is dead. The old `except: text=None` path made a transient
                // Cloud-Function storage hiccup on one monitor tick read as
                // "no heartbeat" for EVERY running job, requeuing them all
                // in the same pass — the synchronized orphan churn (3ef705b2
                // + 724084db both requeued 2026-05-16T04:09:19, restart 11,
                // on freshly-written heartbeat blobs). Fail safe: a read
                // error defers (treat as alive); never let the coordinator's
                // own read failure destroy a running job. A genuinely dead
                // job is still caught when the read succeeds (stale ts) or
                // via the TERMINATED/absent VM path.
                return true;
            }
        };
        let Some(text) = text.filter(|t| !t.is_empty()) else {
            continue;
        };
        let Some(ts) = parse_heartbeat_ts(&text) else {
            continue;
        };
        if now - ts < threshold_seconds {
            return true;
        }
    }
    false
}

/// Extract the in-bucket checkpoint prefix from a training command's
/// `--checkpoint-gcs-uri gs://<bucket>/<prefix>` flag. Returns the part
/// after the bucket (e.g. 'ckpts/qwen3_4b_5k_s0/') or None.
fn ckpt_prefix_from_command(cmd: &str) -> Option<String> {
    if cmd.is_empty() {
        return None;
    }
    let caps = CKPT_URI_RE.captures(cmd)?;
    let uri = caps[1].trim();
    let rest = uri.strip_prefix("gs://")?;
    let (_bucket, prefix) = rest.split_once('/')?;
    if prefix.is_empty() {
        return None;
    }
    Some(prefix.to_string())
}

/// True iff the job's GCS checkpoint directory has a blob written
/// within threshold_seconds.
///
/// A fresh checkpoint write is proof the training process is alive AND
/// productive, and it is immune to the exact failure mode that makes
/// the per-job heartbeat go stale: a multi-GB checkpoint upload
/// saturates the box's outbound network and starves the small heartbeat
/// PUT, so the heartbeat ages past the orphan threshold WHILE the job
/// is demonstrably alive — it is in the middle of writing that very
/// checkpoint. Confirmed live 2026-05-16: job 724084db was requeued
/// 'local agent live but job heartbeat stale (orphan)' at 23:18:29
/// while `[ckpt] sync step 1530` had completed at 23:12 and step
/// 1520->1521 stalled ~1h on the GCS upload; the orphan branch was
/// burning the restart budget (15/20) on healthy checkpoint uploads.
///
/// The newest blob under the checkpoint prefix is the liveness signal:
/// while a multi-GB checkpoint uploads, its shard blobs are
/// continuously updated, so max(updated) stays fresh even mid-upload.
/// A genuinely dead job writes zero new checkpoint blobs ever, so it is
/// still requeued once both the heartbeat and the checkpoint age out.
pub async fn any_job_checkpoint_fresh(
    store: &JobStorage,
    job: &Job,
    threshold_seconds: f64,
) -> bool {
    prefix_has_fresh_blob(
        store,
        ckpt_prefix_from_command(&job.command).as_deref(),
        threshold_seconds,
    )
    .await
}

/// True iff any blob under `prefix` was updated within
/// threshold_seconds. Shared by the job-object and jids-list checkpoint
/// guards. A coordinator-side GCS read/list failure is NOT proof the job
/// is dead, so it fails safe (returns True / defers).
async fn prefix_has_fresh_blob(
    store: &JobStorage,
    prefix: Option<&str>,
    threshold_seconds: f64,
) -> bool {
    let Some(prefix) = prefix.filter(|p| !p.is_empty()) else {
        return false;
    };
    let now = now_unix();
    let infos = match store.list_blobs_with_meta(prefix).await {
        Ok(infos) => infos,
        Err(_) => return true, // fail safe (Python: except Exception -> True)
    };
    let mut newest = 0.0_f64;
    for info in infos {
        let Some(upd) = info.updated else { continue };
        let ts = unix_seconds(upd);
        if ts > newest {
            newest = ts;
        }
    }
    if newest <= 0.0 {
        return false;
    }
    (now - newest) < threshold_seconds
}

/// List jids whose running/ blob has instance_ref == ref, read FRESH
/// at call time. Used by reap_dead_agents as the FINAL safety check
/// immediately before any delete_instance, to defeat the race that
/// burned restart 16 of job 724084db at 2026-05-17T21:26:07: Branch B
/// (never-worked) checked `instance_ref not in active_refs` where
/// active_refs was a cache built at function entry, list_jobs("running")
/// DID NOT return 724084db at that tick (transient listing miss), the
/// gate held, the VM was deleted, and _requeue_jids_after_reap got an
/// empty jids list (_ref_to_jids came from the same cached listing) —
/// so the job was left wedged in running/ pointing at a deleted VM,
/// auto-recovery delayed until heartbeat staled.
///
/// On read-error, returns a sentinel non-empty list so the caller
/// DEFERS (treats VM as in-use) rather than reaping — same fail-safe
/// philosophy as any_job_heartbeat_fresh.
pub async fn fresh_jids_pointing_to_ref(store: &JobStorage, instance_ref: &str) -> Vec<String> {
    match store.list_jobs("running", 0).await {
        Ok(jobs) => jobs
            .iter()
            .filter(|j| j.instance_ref.as_deref() == Some(instance_ref))
            .map(|j| j.job_id.clone())
            .filter(|jid| !jid.is_empty())
            .collect(),
        Err(_) => vec![LIST_FAILED_SENTINEL.to_string()],
    }
}

/// Best-effort fetch of a job's command string from its running/ or
/// queue/ blob, given only the job id (the reaper has jids, not job
/// objects).
async fn job_command_for_jid(store: &JobStorage, jid: &str) -> String {
    for prefix in ["running", "queue"] {
        let text = match store.download_text(&format!("{prefix}/{jid}.json")).await {
            Ok(text) => text,
            // fail-safe handled by caller's defer-on-fresh logic
            Err(_) => return String::new(),
        };
        let Some(text) = text.filter(|t| !t.is_empty()) else {
            continue;
        };
        // Python json.loads(txt).get("command") or ""; a non-object or
        // unparseable blob maps to "" (Python: except -> "").
        return serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| {
                v.get("command")
                    .and_then(|c| c.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_default();
    }
    String::new()
}

/// jids-list variant of any_job_checkpoint_fresh, for the
/// reap_dead_agents defer-guards (Branches A/B/C) which only have job
/// ids, not job objects. True iff ANY job's GCS checkpoint dir has a
/// blob written within threshold_seconds — the same
/// network-saturation-immune proof-of-life as the orphan-branch guard:
/// the multi-GB checkpoint upload that starves the heartbeat IS what
/// produces fresh ckpt blobs. Confirmed live 2026-05-17: job 724084db
/// was reaped 'VM reaped (wedged agent)' restart 16 at 20:42:17 while
/// checkpoint-2480 (17.28 GiB) had finalized 20:31:26 — the wedged
/// reaper's heartbeat-only defer-guard lost the race to the
/// network-starved heartbeat. Branches A/B/C now also consult this.
pub async fn any_job_checkpoint_fresh_jids(
    store: &JobStorage,
    jids: &[String],
    threshold_seconds: f64,
) -> bool {
    for jid in jids {
        if jid.is_empty() {
            continue;
        }
        let prefix = ckpt_prefix_from_command(&job_command_for_jid(store, jid).await);
        if prefix_has_fresh_blob(store, prefix.as_deref(), threshold_seconds).await {
            return true;
        }
    }
    false
}

/// True if the job command kills the `wc agent` process itself
/// (e.g. an upgrade-then-restart maintenance job:
/// `pip install --upgrade ... ; pkill -f "wc agent"`).
///
/// For such a command the agent's disappearance is the SUCCESS
/// condition, not an orphan failure. The agent dies before it can
/// write a COMPLETED status, so the job is stranded in running/ and
/// the orphan-reaper requeues it — which re-runs the kill on the
/// next agent generation, an infinite crash loop. Confirmed live
/// 2026-05-15: job 435b184e crash-looped ubuntu-server
/// (wisent-agent.service n_restarts=7) until removed by operator.
pub fn is_self_terminating_command(cmd: &str) -> bool {
    if cmd.is_empty() {
        return false;
    }
    (cmd.contains("pkill") || cmd.contains("kill ")) && cmd.contains("wc agent")
}

/// If job.command self-terminates the agent, finalize the job as
/// COMPLETED (running/ -> completed/) and return True. Otherwise
/// return False so the caller proceeds with its normal requeue path.
pub async fn finalize_if_self_terminating(
    store: &JobStorage,
    job: &mut Job,
    log_fn: &dyn Fn(&str),
) -> Result<bool, StorageError> {
    if !is_self_terminating_command(&job.command) {
        return Ok(false);
    }
    job.state = job_state::COMPLETED.to_string();
    job.completed_at = Some(isoformat_utc(Utc::now()));
    job.instance_ref = None;
    store.move_job(job, "running", "completed").await?;
    store.cleanup_status(&job.job_id).await?;
    log_fn(&format!(
        "{}: COMPLETED (self-terminating maintenance cmd; \
         agent kill is the success condition, not an orphan)",
        job.job_id
    ));
    Ok(true)
}

/// Build instance_ref -> list[job_id] from store.list_jobs('running').
/// Used by the reaper to find which jobs claim each VM ref before the
/// heartbeat freshness check.
pub async fn build_ref_to_jids(
    store: &JobStorage,
) -> Result<BTreeMap<String, Vec<String>>, StorageError> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for job in store.list_jobs("running", 0).await? {
        if let Some(instance_ref) = job.instance_ref.filter(|r| !r.is_empty()) {
            if !job.job_id.is_empty() {
                out.entry(instance_ref)
                    .or_default()
                    .push(job.job_id.clone());
            }
        }
    }
    Ok(out)
}
