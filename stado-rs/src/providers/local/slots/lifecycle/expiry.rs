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
        // Secrets that cannot be resolved, or a work directory that cannot be
        // examined, keep the hook from running with the cause: run anyway, it
        // ran with an empty secret environment or outside its work directory.
        let prepared = match resolve_job_secret_environment(&job).await {
            Ok(environment) => crate::primitives::presence::present(&work_dir)
                .map(|in_place| (environment, in_place))
                .map_err(|error| error.to_string()),
            Err(error) => Err(format!("its workload secrets cannot be resolved: {error}")),
        };
        match prepared {
            Err(cause) => log_fn(&format!(
                "yield: on-yield hook {} not run: {cause}",
                job.job_id
            )),
            Ok((secret_environment, in_place)) => {
                let mut cmd = tokio::process::Command::new("/bin/sh");
                inherit_safe_agent_environment(&mut cmd);
                cmd.arg("-c")
                    .arg(&hook)
                    .env("WC_JOB_ID", &job.job_id)
                    .env("WC_JOB_PID", pgid.to_string())
                    .envs(secret_environment);
                if in_place {
                    cmd.current_dir(&work_dir);
                }
                match crate::wait::output_async(&mut cmd).await {
                    Ok(out) => {
                        let rc = python_returncode(out.status);
                        if !out.status.success() {
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
        }
    }

    if slot.child.try_wait()?.is_none() {
        log_fn(&format!(
            "yield: {} still running after its yield hook finished — SIGTERM group {pgid}",
            job.job_id
        ));
        // ProcessLookupError parity: the group may have exited between the
        // check and the signal. Any other failure leaves the job running while
        // the wait below begins, so it is logged.
        match nix::sys::signal::killpg(Pid::from_raw(pgid), Signal::SIGTERM) {
            Ok(()) | Err(nix::errno::Errno::ESRCH) => {}
            Err(errno) => log_fn(&format!(
                "yield: SIGTERM to {} (group {pgid}) failed: {errno}",
                job.job_id
            )),
        }
        crate::wait::until(
            crate::wait::Kind::Process,
            format!("job {} to exit after SIGTERM", job.job_id),
            format!("process group {pgid}"),
            slot.child.wait(),
        )
        .await?;
    }

    slot.close_log();

    job.yield_count = job
        .yield_count
        .saturating_add(i64::from(std::num::NonZeroU8::MIN.get()));
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
    if let Err(exc) = upload_output(store, &job, &output_dir).await {
        log_fn(&format!(
            "yield: output upload {} failed (non-fatal): {exc}",
            job.job_id
        ));
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
        // Darwin answers EPERM, not ESRCH, for a group whose only members are
        // zombies. Once the leader has exited no live member was left to
        // signal, and the wait below reaps it; any other EPERM is refused.
        Err(nix::errno::Errno::EPERM) if slot.child.try_wait()?.is_some() => {
            log_fn(&format!(
                "cancelled job process group {pgid} had already exited: SIGTERM found only \
                 exited members (EPERM)"
            ));
            slot.paused = false;
        }
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
    crate::wait::until(
        crate::wait::Kind::Process,
        "the cancelled job to exit after SIGTERM",
        format!("process group {pgid}"),
        slot.child.wait(),
    )
    .await?;
    Ok(())
}
