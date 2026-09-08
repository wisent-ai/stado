//! The two liveness facts a running slot publishes: the operator- and
//! monitor-facing `status/` blob, and the lease renewal on the running job
//! document that actually fences the reaper.

use super::*;

// ---------------------------------------------------------------------------
// status / heartbeat writes
// ---------------------------------------------------------------------------

/// Python `_write_status`. Refusal-without-backend collapsed away (see the
/// module-docs deviation): every Rust backend can write the blob.
pub async fn write_status(
    store: &JobStorage,
    job_id: &str,
    status: &str,
) -> Result<(), StorageError> {
    store
        .upload_text(&format!("status/{job_id}/status"), status)
        .await
}

/// Stamp a fresh `status/<job_id>/heartbeat` blob so the CF monitor sees
/// the workstation slot is alive. Python `_write_heartbeat`.
///
/// Earlier this used `subprocess.run([gsutil, cp, ...], capture_output=True)`
/// which silently swallowed any failure. When gsutil hit a transient auth
/// glitch, network blip, or concurrent-fork ENOMEM, the heartbeat write
/// vanished into the void; the CF monitor saw an old/missing blob, and
/// requeued every workstation job at the 15-minute staleness threshold.
/// Confirmed live on 2026-05-06 (job 01d79e28 had no heartbeat blob despite
/// the slot being live; jobs 4724ae6d/3f16d8b4/24dee60d were yanked from
/// running/ for 'stale heartbeat (local consumer)' in a single 4-second
/// monitor window).
/// Writes go through the storage backend directly (no fork, no swallowed
/// error).
///
/// The pulse is TWO facts now. The blob under `status/` is the operator- and
/// monitor-facing timestamp it always was; the lease renewed on the running
/// job document is what actually fences the reaper, because it is a
/// compare-and-swap on the very object a reap moves. A reaper that read the
/// job a moment ago is holding a version this renewal invalidates, so its
/// move fails instead of requeueing a job that is still executing.
pub async fn write_heartbeat(store: &JobStorage, job_id: &str) -> Result<(), StorageError> {
    // Renew the authoritative fence FIRST. A slow or failed operator-facing
    // status upload must not postpone the CAS that keeps a live execution from
    // being reaped. `false` means the job already left running/, so publishing
    // another pulse beside it would only create a stale liveness signal.
    if !store.renew_running_lease(job_id).await? {
        return Ok(());
    }
    let ts = isoformat_utc(Utc::now());
    store
        .upload_text(
            &format!("status/{job_id}/heartbeat"),
            &format!("RUNNING {ts}"),
        )
        .await
}

/// Stamp status/<job>/heartbeat every HEARTBEAT_INTERVAL_S for as long as
/// the training subprocess is alive — independent of the agent main loop.
/// Python `_start_heartbeat_thread`.
///
/// The loop-coupled write (slots tick, only fires when the agent reaches
/// it) let a loop blocked >1800s on another slot's checkpoint pull / drift
/// check / HF download starve a HEALTHY job's heartbeat, so the CF monitor
/// orphan-requeued it: Llama 3ef705b2 + Qwen3 724084db were both requeued
/// in one monitor pass at 2026-05-15T16:18:42 ('local agent live but job
/// heartbeat stale (orphan)') while training was actively progressing. A
/// task keyed on pid liveness makes the heartbeat mean 'training process
/// alive', not 'agent loop ran recently'.
pub fn start_heartbeat_task(
    store: JobStorage,
    job_id: String,
    pid: i32,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while helpers::pid_alive(pid) {
            tokio::time::sleep(Duration::from_secs(HEARTBEAT_INTERVAL_S)).await;
            if let Err(err) = write_heartbeat(&store, &job_id).await {
                // The coordinator requeues local jobs when their heartbeat
                // goes stale. Silent heartbeat failures leave live jobs looking
                // dead, so make the next failure visible in the agent log.
                eprintln!("[heartbeat] write failed for {job_id}: {err}");
            }
        }
    })
}
