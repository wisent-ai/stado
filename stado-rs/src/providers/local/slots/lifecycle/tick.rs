//! The two pieces of a slot's tick that talk to the outside while the
//! workload is judged: the heartbeat of a running job, and the verification
//! command of one that exited cleanly.

use std::path::Path;

use super::*;

/// A running slot's periodic work, once per agent tick: the peak-VRAM
/// attribution, the status blob and the streamed log.
pub(super) async fn running_tick(
    slot: &mut ActiveSlot,
    store: &JobStorage,
    job_id: &str,
    pid: i32,
    log_fn: &mut dyn FnMut(&str),
) -> Result<(), StorageError> {
    let used = gpu::smi_job_used_gb(pid).await;
    if used > slot.slot.peak_vram_gb {
        slot.slot.peak_vram_gb = used;
    }
    if slot.paused {
        return Ok(());
    }
    if !write_heartbeat(store, job_id).await? {
        take_back(store, slot, job_id, log_fn).await?;
    }
    let expected_work_dir = job_work_dir(job_id)?;
    if !work_dir_is_directory(&expected_work_dir) {
        slot.workdir_missing = true;
        log_fn(&workdir_missing_diagnostic(
            job_id,
            "heartbeat",
            &expected_work_dir,
        ));
    } else {
        // Stream the in-progress command_output.log from the queue-owned
        // persistent job tree on each heartbeat. An existing but empty log is
        // intentionally different from the missing-tree diagnostic above.
        let log_path = expected_work_dir.join("output/command_output.log");
        if log_path.exists() {
            let upload = async {
                let mut bytes = tokio::fs::read(&log_path).await?;
                let secrets = output_redactions(&slot.slot.job).await?;
                redact_secret_bytes(&mut bytes, &secrets);
                store
                    .upload_bytes(
                        &format!("status/{job_id}/output/command_output.log"),
                        &bytes,
                    )
                    .await
            };
            // The upload finishes or the store refuses it; nothing bounds it
            // by a clock, so a slow object store still receives the log.
            if let Err(exc) = upload.await {
                log_fn(&format!(
                    "heartbeat log upload failed for {job_id}: {}",
                    head_chars(&exc.to_string(), 160)
                ));
            }
        }
    }
    Ok(())
}

/// Take back a job this slot still runs after a reaper put it back in the
/// queue: its lease lapsed while the process ran. Starting it again would run
/// the same work twice side by side in one work directory, so the queued
/// record moves back to running/ as the attempt already under way — this
/// slot's start time and a fresh lease the slot's heartbeat renews from then
/// on — and nothing is started. A job that left running/ for anywhere but the
/// queue (cancelled, finished elsewhere) is not this slot's to take, and the
/// heartbeat task has already said it was lost.
async fn take_back(
    store: &JobStorage,
    slot: &ActiveSlot,
    job_id: &str,
    log_fn: &mut dyn FnMut(&str),
) -> Result<(), StorageError> {
    let Some(mut record) = store.read_job("queue", job_id).await? else {
        return Ok(());
    };
    let process = match slot.slot.pid {
        Some(pid) => format!("process group {pid}"),
        None => "a process whose pid was not recorded".to_string(),
    };
    let age = slot.started_mono.elapsed().as_secs();
    let Some(promise) = crate::providers::local::slots::lease_promise() else {
        log_fn(&format!(
            "{job_id}: already running here as {process} for {age} s and put back in the queue; \
             it cannot be taken back because no agent poll period is declared in this process, \
             so its lease would carry no promise; no second process is started"
        ));
        return Ok(());
    };
    record.state = crate::models::job_state::RUNNING.to_string();
    record.started_at = slot.slot.job.started_at.clone();
    record.instance_ref = slot.slot.job.instance_ref.clone();
    record.worker_allocation = slot.slot.job.worker_allocation.clone();
    if store.claim_queued_job(&record, promise).await? {
        log_fn(&format!(
            "{job_id}: already running here as {process} for {age} s: its lease lapsed while it \
             ran and it was put back in the queue; the running process took the job back and no \
             second process was started"
        ));
    } else {
        log_fn(&format!(
            "{job_id}: already running here as {process} for {age} s, but another worker or a \
             cancellation took its queued record first; no second process is started here, and \
             what this process finishes is published beside a record that no longer expects it"
        ));
    }
    Ok(())
}

/// Run a cleanly exited job's verification command (see
/// `Job.verify_command`) in the workload's own directory, and answer the
/// exit code the job is recorded with when verification fails: the secret
/// environment unresolvable, the command's own code plus 1000, or 1999 when
/// it could not start. The command defines its own failure conditions; the
/// runner imposes no wall-clock cap.
pub(super) async fn verification_failure(
    job: &Job,
    verify_cmd: &str,
    work_dir: &Path,
    job_id: &str,
    log_fn: &mut dyn FnMut(&str),
) -> Option<i32> {
    let mut failure = None;
    let secret_environment = match resolve_job_secret_environment(job).await {
        Ok(environment) => environment,
        Err(_) => {
            failure = Some(i32::MAX);
            log_fn(&format!(
                "verify_command secret resolution failed for {job_id}"
            ));
            BTreeMap::new()
        }
    };
    let mut command = tokio::process::Command::new("/bin/sh");
    inherit_safe_agent_environment(&mut command);
    match crate::wait::output_async(
        command
            .arg("-c")
            .arg(verify_cmd)
            .current_dir(work_dir)
            .envs(secret_environment),
    )
    .await
    {
        Ok(out) => {
            let vrc = python_returncode(out.status);
            if vrc != 0 {
                failure = Some(1000 + vrc);
                log_fn(&format!("verify_command failed for {job_id}: rc={vrc}"));
            }
        }
        Err(_) => {
            failure = Some(1999);
            log_fn(&format!("verify_command failed to start for {job_id}"));
        }
    }
    failure
}
