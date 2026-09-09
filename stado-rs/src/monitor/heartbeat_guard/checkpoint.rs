//! The GCS checkpoint prefix: where a training command writes it, and
//! whether anything under it was written recently enough to prove the
//! job alive even while the heartbeat is network-starved.

use std::sync::LazyLock;

use regex::Regex;

use crate::models::Job;
use crate::queue::JobStorage;

use super::{now_unix, unix_seconds};

static CKPT_URI_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"--checkpoint-gcs-uri\s+(\S+)").expect("static regex compiles"));

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
