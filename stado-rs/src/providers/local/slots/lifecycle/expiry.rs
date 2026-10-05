//! The two ways a slot's process stops before it finishes: the cooperative
//! yield back to the queue, and the termination of a job that has already been
//! cancelled elsewhere.

use super::*;

// ---------------------------------------------------------------------------
// request_yield
// ---------------------------------------------------------------------------

/// Cooperatively yield a running slot to free its VRAM for higher-priority
/// work. Returns true once the job has been requeued.
/// Python `request_yield`.
///
/// Sequence:
///   1. Run the job's yield_command (the save-and-sync hook) in the job
///      workdir with WC_JOB_PID set to the process-group leader, so the hook
///      can signal the job, persist state + artifacts externally, and let it
///      exit. The hook runs to completion.
///   2. If the job is still running when the hook has finished, send its
///      process group SIGTERM (logged: the hook did not stop it) and wait for
///      it to exit.
///   3. Requeue: running -> queue, state QUEUED, yield_count++, clear
///      instance_ref/started_at. NOT marked FAILED — resume is the job's own
///      business (checkpoint pull, server-side state, ...).
///
/// The slot's process was started with process_group(0), so its pid is the
/// process-group id.
pub async fn request_yield(
    mut slot: ActiveSlot,
    store: &JobStorage,
    log_fn: &mut dyn FnMut(&str),
) -> Result<bool, StorageError> {
    let pgid = slot.pid();
    let mut job = slot.slot.job.clone();
    let hook = job.yield_command.trim().to_string();
    let work_dir = job_work_dir(&job.job_id)?;
    log_fn(&format!(
        "yield: requesting yield of {} (pgid={pgid})",
        job.job_id
    ));

    if !hook.is_empty() {
        let secret_environment = match resolve_job_secret_environment(&job).await {
            Ok(environment) => environment,
            Err(_) => {
                log_fn(&format!(
                    "yield: workload secret resolution failed for {}",
                    job.job_id
                ));
                BTreeMap::new()
            }
        };
        let mut cmd = tokio::process::Command::new("/bin/sh");
        inherit_safe_agent_environment(&mut cmd);
        cmd.arg("-c")
            .arg(&hook)
            .env("WC_JOB_ID", &job.job_id)
            .env("WC_JOB_PID", pgid.to_string())
            .envs(secret_environment);
        if work_dir.exists() {
            cmd.current_dir(&work_dir);
        }
        match cmd.output().await {
            Ok(out) => {
                let rc = python_returncode(out.status);
                if rc != 0 {
                    log_fn(&format!(
                        "yield: on-yield hook {} failed with rc={rc}",
                        job.job_id
                    ));
                }
            }
            Err(exc) => log_fn(&format!(
                "yield: on-yield hook {} raised: {exc}",
                job.job_id
            )),
        }
    }

    if slot.child.try_wait()?.is_none() {
        log_fn(&format!(
            "yield: {} still running after its yield hook finished — SIGTERM group {pgid}",
            job.job_id
        ));
        // ProcessLookupError parity: the group may have exited between the
        // check and the signal.
        let _ = nix::sys::signal::killpg(Pid::from_raw(pgid), Signal::SIGTERM);
        slot.child.wait().await?;
    }

    slot.close_log();

    job.yield_count += 1;
    job.state = job_state::QUEUED.to_string();
    job.instance_ref = None;
    job.started_at = None;
    write_status(
        store,
        &job.job_id,
        &format!("YIELDED {}", isoformat_utc(Utc::now())),
    )
    .await?;
    let output_dir = work_dir.join("output");
    if output_dir.exists() {
        if let Err(exc) = upload_output(store, &job, &output_dir).await {
            log_fn(&format!(
                "yield: output upload {} failed (non-fatal): {exc}",
                job.job_id
            ));
        }
    }
    // running -> queue (NOT a terminal state, so the tracking tombstone hook
    // is a no-op and the CF monitor leaves it alone once out of running/).
    store.move_job(&job, "running", "queue").await?;
    log_fn(&format!(
        "yield: {} requeued (yield_count={})",
        job.job_id, job.yield_count
    ));
    Ok(true)
}

pub(super) async fn terminate_cancelled_slot(
    slot: &mut ActiveSlot,
    log_fn: &mut dyn FnMut(&str),
) -> std::io::Result<()> {
    let pgid = slot.pid();
    log_fn(&format!(
        "cancelled job process group {pgid}: sending SIGTERM and waiting for it to exit"
    ));
    match nix::sys::signal::killpg(Pid::from_raw(pgid), Signal::SIGTERM) {
        Ok(()) | Err(nix::errno::Errno::ESRCH) => {}
        Err(error) => {
            return Err(std::io::Error::other(format!(
                "SIGTERM to cancelled process group {pgid}: {error}"
            )))
        }
    }
    if slot.paused {
        match nix::sys::signal::killpg(Pid::from_raw(pgid), Signal::SIGCONT) {
            Ok(()) | Err(nix::errno::Errno::ESRCH) => {}
            Err(error) => {
                return Err(std::io::Error::other(format!(
                    "SIGCONT to cancelled paused process group {pgid}: {error}"
                )))
            }
        }
        slot.paused = false;
    }
    slot.child.wait().await?;
    Ok(())
}
