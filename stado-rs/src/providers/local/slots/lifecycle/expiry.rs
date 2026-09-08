//! The two ways a slot's process stops before it finishes: the cooperative
//! yield back to the queue, bounded by the job's own grace, and the
//! termination of a job that has already been cancelled elsewhere.

use super::*;

// ---------------------------------------------------------------------------
// request_yield
// ---------------------------------------------------------------------------

/// Cooperatively yield a running slot to free its VRAM for higher-priority
/// work. Returns true once the job has been requeued.
/// Python `request_yield`.
///
/// Sequence (total bounded by job.yield_grace_seconds):
///   1. Run the job's yield_command (the save-and-sync hook) in the job
///      workdir with WC_JOB_PID set to the process-group leader, so the hook
///      can signal the job, persist state + artifacts externally, and let it
///      exit.
///   2. Wait for the process to exit on its own within the remaining grace.
///   3. SIGKILL the whole process group only if the grace is blown (logged
///      loudly — a timed-out yield means the hook didn't actually stop it).
///   4. Requeue: running -> queue, state QUEUED, yield_count++, clear
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
    // Python `int(getattr(job, "yield_grace_seconds", 120) or 120)`: 0 -> 120.
    let grace = if job.yield_grace_seconds != 0 {
        job.yield_grace_seconds
    } else {
        DEFAULT_YIELD_GRACE_S
    };
    let hook = job.yield_command.trim().to_string();
    let work_dir = job_work_dir(&job.job_id)?;
    let deadline = Instant::now() + Duration::from_secs(grace.max(0) as u64);
    log_fn(&format!(
        "yield: requesting yield of {} (grace={grace}s, pgid={pgid})",
        job.job_id
    ));

    if !hook.is_empty() {
        let remaining = deadline
            .saturating_duration_since(Instant::now())
            .as_secs()
            .max(1);
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
            .envs(secret_environment)
            // A timed-out hook is killed (Python subprocess.run timeout
            // semantics: kill the direct child, reap, raise).
            .kill_on_drop(true);
        if work_dir.exists() {
            cmd.current_dir(&work_dir);
        }
        match tokio::time::timeout(Duration::from_secs(remaining), cmd.output()).await {
            Ok(Ok(out)) => {
                let rc = python_returncode(out.status);
                if rc != 0 {
                    log_fn(&format!(
                        "yield: on-yield hook {} failed with rc={rc}",
                        job.job_id
                    ));
                }
            }
            Ok(Err(exc)) => log_fn(&format!(
                "yield: on-yield hook {} raised: {exc}",
                job.job_id
            )),
            Err(_) => log_fn(&format!(
                "yield: on-yield hook {} exceeded grace; terminating",
                job.job_id
            )),
        }
    }

    loop {
        if slot.child.try_wait()?.is_some() {
            break;
        }
        if Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    if slot.child.try_wait()?.is_none() {
        log_fn(&format!(
            "yield: {} still alive after grace — SIGKILL group {pgid}",
            job.job_id
        ));
        // ProcessLookupError parity: the group may have exited between the
        // poll and the signal.
        let _ = nix::sys::signal::killpg(Pid::from_raw(pgid), Signal::SIGKILL);
        let _ = tokio::time::timeout(Duration::from_secs(10), slot.child.wait()).await;
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
    let _ = nix::sys::signal::killpg(Pid::from_raw(pgid), Signal::SIGTERM);
    match tokio::time::timeout(
        Duration::from_secs(crate::constants::POLL_INTERVAL_S),
        slot.child.wait(),
    )
    .await
    {
        Ok(result) => {
            result?;
        }
        Err(_) => {
            log_fn(&format!(
                "cancelled job process group {pgid} ignored SIGTERM; sending SIGKILL"
            ));
            let _ = nix::sys::signal::killpg(Pid::from_raw(pgid), Signal::SIGKILL);
            slot.child.wait().await?;
        }
    }
    Ok(())
}
