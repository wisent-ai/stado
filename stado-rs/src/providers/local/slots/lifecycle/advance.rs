//! One tick of a live slot: the duplicate/cancellation drop, Vast
//! pause/resume, the heartbeat and streamed log while it runs, and the
//! verification, classification and durable terminal transition once its
//! process has exited.

use super::*;

// ---------------------------------------------------------------------------
// advance_slot
// ---------------------------------------------------------------------------

/// Advance one slot. Python `advance_slot` — returns [`SlotOutcome::Running`]
/// while the job runs, [`SlotOutcome::Done`] once completed/failed/dropped.
pub async fn advance_slot(
    mut slot: ActiveSlot,
    store: &JobStorage,
    sizing: &Sizing,
    vast_active: bool,
    log_fn: &mut dyn FnMut(&str),
) -> Result<SlotOutcome, StorageError> {
    let pid = slot.pid();
    let job_id = slot.slot.job.job_id.clone();
    for terminal_prefix in ["uploaded", "completed", "cancelled"] {
        if store.read_job(terminal_prefix, &job_id).await?.is_some() {
            terminate_cancelled_slot(&mut slot, log_fn).await?;
            slot.close_log();
            let output_dir = job_work_dir(&job_id)?.join("output");
            if output_dir.exists() {
                if let Err(error) = upload_output(store, &slot.slot.job, &output_dir).await {
                    log_fn(&format!(
                        "cancelled job artifact upload failed for {job_id}: {error}"
                    ));
                }
            }
            let _ = store.delete_job("running", &job_id).await;
            log_fn(&format!(
                "drop duplicate running {job_id}: already in {terminal_prefix}/"
            ));
            return Ok(SlotOutcome::Done);
        }
    }
    if !slot.paused && vast_active {
        log_fn(&format!("Renter detected, pausing job {job_id}"));
        nix::sys::signal::kill(Pid::from_raw(pid), Signal::SIGSTOP)
            .map_err(|e| StorageError::Other(format!("SIGSTOP pid {pid}: {e}")))?;
        slot.paused = true;
    } else if slot.paused && !vast_active {
        log_fn(&format!("Renter gone, resuming job {job_id}"));
        nix::sys::signal::kill(Pid::from_raw(pid), Signal::SIGCONT)
            .map_err(|e| StorageError::Other(format!("SIGCONT pid {pid}: {e}")))?;
        slot.paused = false;
    }
    // `reap` and not `child.try_wait`: observing the exit is what releases the
    // janitor's shared cleanup hold, so nothing below this line can retain it.
    let Some(exit_status) = slot.reap(log_fn)? else {
        // Still running: refresh the peak-VRAM attribution and, on the
        // heartbeat interval, the status blob + streamed log.
        let used = gpu::smi_job_used_gb(pid).await;
        if used > slot.slot.peak_vram_gb {
            slot.slot.peak_vram_gb = used;
        }
        if !slot.paused && slot.last_hb.elapsed() > Duration::from_secs(HEARTBEAT_INTERVAL_S) {
            write_heartbeat(store, &job_id).await?;
            let expected_work_dir = job_work_dir(&job_id)?;
            if !work_dir_is_directory(&expected_work_dir) {
                slot.workdir_missing = true;
                log_fn(&workdir_missing_diagnostic(
                    &job_id,
                    "heartbeat",
                    &expected_work_dir,
                ));
            } else {
                // Stream the in-progress command_output.log from the
                // queue-owned persistent job tree on each heartbeat. An
                // existing but empty log is intentionally different from the
                // missing-tree diagnostic above.
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
                    match tokio::time::timeout(Duration::from_secs(10), upload).await {
                        Ok(Ok(())) => {}
                        Ok(Err(exc)) => {
                            log_fn(&format!(
                                "heartbeat log upload failed for {job_id}: {}",
                                head_chars(&exc.to_string(), 160)
                            ));
                        }
                        Err(_) => {
                            log_fn(&format!(
                                "heartbeat log upload failed for {job_id}: timed out after 10s"
                            ));
                        }
                    }
                }
            }
            slot.last_hb = Instant::now();
        }
        return Ok(SlotOutcome::Running(slot));
    };

    let workload_exit_code = python_returncode(exit_status);
    let mut ret = workload_exit_code;
    let expected_work_dir = job_work_dir(&job_id)?;
    if !work_dir_is_directory(&expected_work_dir) {
        slot.workdir_missing = true;
    }
    if slot.workdir_missing {
        log_fn(&workdir_missing_diagnostic(
            &job_id,
            "finalization",
            &expected_work_dir,
        ));
    }
    let mut verification_failed = false;
    let verify_cmd = verify_command(&slot.slot.job);
    if ret == 0 && !slot.workdir_missing && !verify_cmd.is_empty() {
        // Verification hook — see Job.verify_command docstring. Runs in
        // the same workdir as the original command. Non-zero exit
        // reverses the COMPLETED→FAILED. The verify command must define
        // its own clear failure conditions; the runner does not impose
        // a wall-clock cap.
        let secret_environment = match resolve_job_secret_environment(&slot.slot.job).await {
            Ok(environment) => environment,
            Err(_) => {
                ret = i32::MAX;
                verification_failed = true;
                log_fn(&format!(
                    "verify_command secret resolution failed for {job_id}"
                ));
                BTreeMap::new()
            }
        };
        let mut command = tokio::process::Command::new("/bin/sh");
        inherit_safe_agent_environment(&mut command);
        match command
            .arg("-c")
            .arg(&verify_cmd)
            .current_dir(&expected_work_dir)
            .envs(secret_environment)
            .output()
            .await
        {
            Ok(out) => {
                let vrc = python_returncode(out.status);
                if vrc != 0 {
                    ret = 1000 + vrc;
                    verification_failed = true;
                    log_fn(&format!("verify_command failed for {job_id}: rc={vrc}"));
                }
            }
            Err(_) => {
                ret = 1999;
                verification_failed = true;
                log_fn(&format!("verify_command failed to start for {job_id}"));
            }
        }
    }
    // Close the log file BEFORE uploading. Earlier this was deferred
    // until the bottom of the branch, after upload_output ran — so
    // buffered writes from the subprocess weren't flushed to disk
    // when the upload captured the file, producing empty/truncated
    // command_output.log uploads. Confirmed live on 2026-05-06: 3
    // gpt-oss-20b "completions" had zero-byte logs despite the
    // subprocess running.
    slot.close_log();
    let terminal_failed = ret != 0 || slot.workdir_missing;
    let status = if terminal_failed {
        format!("FAILED exit={ret}")
    } else {
        "COMPLETED".to_string()
    };
    let mut job = slot.slot.job.clone();
    job.state = if terminal_failed {
        job_state::FAILED.to_string()
    } else {
        job_state::COMPLETED.to_string()
    };
    let output_dir = expected_work_dir.join("output");
    let ts = isoformat_utc(Utc::now());
    // The workload's own last words, read before the failure record is
    // written rather than after it, because they ARE the failure record. This
    // used to be computed below for OOM classification only, while `job.error`
    // said "inspect the redacted command output" and the agent's own log line
    // printed that sentence under the name `error_tail=`.
    //
    // On 2026-08-31 fifteen jobs on charless-mac-mini failed with that
    // sentence. Their output said what happened: CuaDriver panicked on
    // `+[NSPasteboard generalPasteboard]` returning NULL and never created
    // `probierz.sock`. The queue record said nothing, so the failures were
    // read as a missing Accessibility grant — which `stado host gui-automation
    // status` reports as `granted` on that host — and an hour went into a
    // permission that was never the problem.
    let classification_error = if job.state == job_state::FAILED && !slot.workdir_missing {
        redacted_tail(
            &job,
            &output_dir.join("command_output.log"),
            "4096".parse().expect("static error-tail size"),
        )
        .await?
    } else {
        String::new()
    };
    if terminal_failed {
        job.failed_at = Some(ts);
        // Collapsed to one line and bounded: this field is read in tables and
        // in one-line log records, and a multi-line JSON blob there is as
        // unreadable as no detail at all.
        let said = classification_error
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        job.error = Some(if slot.workdir_missing {
            let missing = format!(
                "workdir_missing expected_path={} workload_exit_code={workload_exit_code}",
                expected_work_dir.display()
            );
            match tail_chars(&said, 400) {
                said if said.trim().is_empty() => missing,
                said => format!("{missing}; captured_output: {said}"),
            }
        } else {
            let what = if verification_failed {
                "verification command failed"
            } else {
                "workload exited unsuccessfully"
            };
            match tail_chars(&said, 400) {
                said if said.trim().is_empty() => format!("{what} and wrote no output"),
                said => format!("{what}: {said}"),
            }
        });
    } else {
        job.completed_at = Some(ts);
    }
    // Artifacts become durable before the terminal transition. A storage
    // failure retains the running record so finalization can be retried;
    // success and failure therefore expose the same result contract.
    job.peak_vram_gb = job.peak_vram_gb.max(slot.slot.peak_vram_gb);
    // Stamp the per-GPU-probe marker: this agent is 0.4.241+,
    // so smi_job_used_gb measured the MAX single-GPU footprint
    // (grouped by gpu_uuid), not a cross-GPU sum. observed_vram_gb
    // trusts only flagged peaks, so legacy summed records can no
    // longer poison the model max().
    job.peak_vram_per_gpu = true;
    if job.state == job_state::FAILED
        && sizing
            .escalate_on_oom(store, &mut job, &classification_error)
            .await?
    {
        log_fn(&format!(
            "Job {job_id} OOM-escalated to gpu_mem_gb={}; requeued",
            job.gpu_mem_gb
        ));
        return Ok(SlotOutcome::Done);
    }
    if output_dir.exists() {
        if let Err(error) = upload_output(store, &job, &output_dir).await {
            log_fn(&format!(
                "terminal artifact upload failed for {job_id}; retaining running state for retry: {error}"
            ));
            // `Running` here means "finalize me again next tick", NOT "a
            // workload is executing" — the process exited above. The retry is
            // unbounded on purpose, so nothing scoped to a live workload may
            // ride along with it; `reap` has already released the janitor
            // hold. See [`release_hold_for_exited_workload`].
            return Ok(SlotOutcome::Running(slot));
        }
    }
    write_status(store, &job_id, &status).await?;
    let to_prefix = job.state.clone();
    store.move_job(&job, "running", &to_prefix).await?;
    // Mirror to job.output_uri if set. Runs for both COMPLETED and
    // FAILED so debugging logs and partial artifacts also land at
    // the caller's project URI. Failure here is logged, not raised
    // — canonical status/<id>/output/ is already written.
    mirror_to_output_uri(store, &job, log_fn).await;
    // log_file already flushed+closed above before upload_output.
    if job.state == job_state::FAILED {
        log_fn(&format!(
            "Job {job_id} failed ret={ret} error_tail={}",
            tail_chars(job.error.as_deref().unwrap_or(""), 500)
        ));
    } else {
        log_fn(&format!("Job {job_id} {}", job.state));
    }
    Ok(SlotOutcome::Done)
}
