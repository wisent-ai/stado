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
    write_heartbeat(store, job_id).await?;
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
        &mut command
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
