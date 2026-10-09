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
/// requeued every workstation job at the 15-minute staleness threshold —
/// live slots with no heartbeat blob, and whole batches of jobs yanked from
/// running/ for 'stale heartbeat (local consumer)' in a single monitor
/// window.
/// Writes go through the storage backend directly (no fork, no swallowed
/// error).
///
/// The pulse is TWO facts now. The blob under `status/` is the operator- and
/// monitor-facing timestamp it always was; the lease renewed on the running
/// job document is what actually fences the reaper, because it is a
/// compare-and-swap on the very object a reap moves. A reaper that read the
/// job a moment ago is holding a version this renewal invalidates, so its
/// move fails instead of requeueing a job that is still executing.
///
/// Answers whether the lease was renewed: `false` means the job is no longer
/// a running document, so this execution has lost it and no pulse is written.
pub async fn write_heartbeat(store: &JobStorage, job_id: &str) -> Result<bool, StorageError> {
    let Some(promise) = super::lease_promise() else {
        return Err(StorageError::Other(format!(
            "no agent poll period is declared in this process, so the lease of {job_id} \
             cannot be renewed with a promise"
        )));
    };
    // Renew the authoritative fence FIRST. A slow or failed operator-facing
    // status upload must not postpone the CAS that keeps a live execution from
    // being reaped. `false` means the job already left running/, so publishing
    // another pulse beside it would only create a stale liveness signal.
    if !store.renew_running_lease(job_id, promise).await? {
        return Ok(false);
    }
    let ts = isoformat_utc(Utc::now());
    store
        .upload_text(
            &format!("status/{job_id}/heartbeat"),
            &format!("RUNNING {ts}"),
        )
        .await?;
    Ok(true)
}

/// Renew the job's lease on the agent's poll period for as long as the
/// training subprocess is alive — independent of the agent main loop.
/// Python `_start_heartbeat_thread`.
///
/// The main loop can be busy downloading another slot's inputs or checking
/// drift while this process continues working. Key heartbeats to process
/// liveness so that unrelated loop work cannot make a live job look orphaned.
/// Every renewal is measured (how late after its period it began, how long
/// its write took), and the next promise covers the worst of it.
pub fn start_heartbeat_task(
    store: JobStorage,
    job_id: String,
    pid: i32,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let Some(poll) = crate::providers::local::agent::POLL.get().copied() else {
            eprintln!(
                "[heartbeat] {job_id}: no agent poll period is set in this process, so no \
                 heartbeat is written for it"
            );
            return;
        };
        let mut previous = std::time::Instant::now();
        let mut lost = false;
        while helpers::pid_alive(pid) {
            tokio::time::sleep(poll).await;
            let began = std::time::Instant::now();
            let result = write_heartbeat(&store, &job_id).await;
            super::record_renewal(poll, Some(began - previous), began.elapsed());
            previous = began;
            match result {
                // The coordinator requeues local jobs when their lease
                // passes. Silent heartbeat failures leave live jobs looking
                // dead, so make the next failure visible in the agent log.
                Err(err) => eprintln!("[heartbeat] write failed for {job_id}: {err}"),
                // A workload still running whose job left running/ was
                // requeued or moved under it: whatever it finishes is
                // published beside a record that no longer expects it.
                Ok(false) if !lost => {
                    lost = true;
                    eprintln!(
                        "[heartbeat] lease of {job_id} not renewed: running/{job_id}.json is no \
                         longer a running document while its workload (pid {pid}) still runs"
                    );
                }
                Ok(_) => {}
            }
        }
        eprintln!("[heartbeat] {job_id}: workload pid {pid} ended; lease renewal stopped");
    })
}
