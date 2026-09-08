//! The claim and the spawn: every refusal that leaves a job queued for
//! another host, the atomic queue handoff, and the process group the workload
//! is given once this agent owns the record.

use super::*;

// ---------------------------------------------------------------------------
// start_slot
// ---------------------------------------------------------------------------

/// Errors before or after the queue claim are agent failures. A claim error is
/// scoped to one queued job and may be reported while the scan continues.
#[derive(Debug)]
pub enum StartSlotError {
    Claim(StorageError),
    Other(StorageError),
}

impl From<StorageError> for StartSlotError {
    fn from(error: StorageError) -> Self {
        Self::Other(error)
    }
}

impl From<std::io::Error> for StartSlotError {
    fn from(error: std::io::Error) -> Self {
        Self::Other(error.into())
    }
}

/// Spawn a subprocess for `job`, register it in 'running' state, return the
/// slot. Python `start_slot`.
///
/// `gpu_uuid` is the board the caller admitted this job against, or None when
/// the host has a single accelerator or none at all. When it is set, the child
/// gets `CUDA_VISIBLE_DEVICES=<uuid>` unless the job's own command already
/// decides that: without it every job defaults to device 0, so on a two-card
/// host two admitted slots pile onto one board while the other stays empty.
///
/// Returns None when a dedupe/refusal check fires or apt-install refuses —
/// the caller leaves the job in queue/ for another agent to claim (or the
/// job was already moved/dropped by the check itself).
pub async fn start_slot(
    store: &JobStorage,
    job: Job,
    hostname: &str,
    log_fn: &mut dyn FnMut(&str),
    kind: &str,
    gpu_uuid: Option<&str>,
) -> Result<Option<ActiveSlot>, StartSlotError> {
    let job_id = job.job_id;
    let Some(mut job) = store.read_job("queue", &job_id).await? else {
        log_fn(&format!("claim lost for {job_id}: queued record is absent"));
        return Ok(None);
    };
    let cmd = job.command.clone();
    if activation_extraction_must_share_gpu(&cmd) {
        job.exclusive = false;
    }
    for terminal_prefix in ["uploaded", "completed", "cancelled"] {
        if store
            .read_job(terminal_prefix, &job.job_id)
            .await?
            .is_some()
        {
            store.delete_job("queue", &job.job_id).await?;
            log_fn(&format!(
                "drop duplicate queued {}: already in {terminal_prefix}/",
                job.job_id
            ));
            return Ok(None);
        }
    }
    // Placement by what this host can READ, before the claim. Declining leaves
    // the job in queue/ for a host whose agent holds the grant, which is what
    // `Ok(None)` means everywhere else in this function; failing it here would
    // destroy a job that another host could have run.
    if let Err(reason) = secrets_resolvable_here(&job).await {
        log_fn(&format!(
            "decline {}: {reason}; leaving it queued for a host that can resolve it",
            job.job_id
        ));
        return Ok(None);
    }
    let reason = deprecated_activation_command_reason(&cmd);
    if !reason.is_empty() {
        job.state = job_state::FAILED.to_string();
        job.failed_at = Some(isoformat_utc(Utc::now()));
        job.error = Some(reason.to_string());
        store.move_job(&job, "queue", "failed").await?;
        log_fn(&format!("refuse {}: {reason}", job.job_id));
        return Ok(None);
    }
    if !install_apt_packages(&job, kind, log_fn).await {
        return Ok(None);
    }
    let raw_refusal = raw_active_disk_refusal(&cmd);
    if !raw_refusal.is_empty() {
        log_fn(&format!("refuse {}: {raw_refusal}", job.job_id));
        return Ok(None);
    }
    let work_dir = super::disk_cleanup::queue_workdirs::create_work_dir(&job.job_id)?;
    let artifact_inputs_json = canonical_json(&Value::Object(job.resolved_input_artifacts.clone()));
    let artifact_inputs_file = work_dir.join("artifacts.json");
    let mut artifact_inputs = open_agent_reserved_file(&artifact_inputs_file)?;
    artifact_inputs.write_all(artifact_inputs_json.as_bytes())?;
    drop(artifact_inputs);
    if let Err(error) =
        materialize_stado_inputs(store, &job.resolved_input_artifacts, &work_dir).await
    {
        job.state = job_state::FAILED.to_string();
        job.failed_at = Some(isoformat_utc(Utc::now()));
        job.error = Some(format!("input materialization failed: {error}"));
        store.move_job(&job, "queue", "failed").await?;
        log_fn(&format!("refuse {}: {error}", job.job_id));
        return Ok(None);
    }
    let secret_environment = match resolve_job_secret_environment(&job).await {
        Ok(environment) => environment,
        Err(error) => {
            job.state = job_state::FAILED.to_string();
            job.failed_at = Some(isoformat_utc(Utc::now()));
            job.error = Some(format!("workload secret resolution failed: {error}"));
            store.move_job(&job, "queue", "failed").await?;
            log_fn(&format!("refuse {}: {error}", job.job_id));
            return Ok(None);
        }
    };
    let log_file = open_agent_reserved_file(&work_dir.join("output/command_output.log"))?;
    let stdout_file = log_file.try_clone()?;
    let stderr_file = log_file.try_clone()?;
    job.state = job_state::RUNNING.to_string();
    job.started_at = Some(isoformat_utc(Utc::now()));
    job.instance_ref = Some(format!("local@{hostname}"));
    let claimed = store
        .claim_queued_job(&job)
        .await
        .map_err(StartSlotError::Claim)?;
    if !claimed {
        log_fn(&format!(
            "claim lost for {}: another worker or cancellation won",
            job.job_id
        ));
        return Ok(None);
    }
    write_status(
        store,
        &job.job_id,
        &format!("RUNNING {}", isoformat_utc(Utc::now())),
    )
    .await?;
    let full_command = build_job_command(&job);
    let mut command = tokio::process::Command::new("/bin/sh");
    inherit_safe_agent_environment(&mut command);
    apply_job_runtime_environment(&mut command, &job);
    command
        .arg("-c")
        .arg(&full_command)
        .current_dir(&work_dir)
        .env("WC_JOB_ID", &job.job_id)
        .env("WC_ARTIFACT_INPUTS_JSON", &artifact_inputs_json)
        .env("WC_ARTIFACT_INPUTS_FILE", &artifact_inputs_file)
        .envs(secret_environment)
        .stdout(std::process::Stdio::from(stdout_file))
        // subprocess.STDOUT parity: stderr lands in the same log file.
        .stderr(std::process::Stdio::from(stderr_file))
        // Own session/process group so a cooperative yield (request_yield)
        // can signal the WHOLE job tree via the group, and SIGKILL it
        // cleanly if the grace is blown — without that, killing only the
        // shell pid would orphan the GPU process and never free its VRAM.
        // The existing Vast SIGSTOP/SIGCONT still target the root pid
        // directly, so their behavior is unchanged.
        .process_group(0);
    // The job's own command wins: a workload that sets CUDA_VISIBLE_DEVICES
    // (sharding across boards, or picking one deliberately) has already made
    // this decision, and overriding it would silently change what it runs on.
    if let Some(uuid) = gpu_uuid {
        if !full_command.contains("CUDA_VISIBLE_DEVICES") {
            command.env("CUDA_VISIBLE_DEVICES", uuid);
            log_fn(&format!("placed {} on {uuid}", job.job_id));
        }
    }
    let child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            job.state = job_state::FAILED.to_string();
            job.failed_at = Some(isoformat_utc(Utc::now()));
            job.error = Some(format!("workload process failed to spawn: {error}"));
            store.move_job(&job, "running", "failed").await?;
            log_fn(&format!("refuse {}: {error}", job.job_id));
            return Err(error.into());
        }
    };
    let pid = child.id().expect("freshly spawned child has a pid") as i32;
    log_fn(&format!(
        "Started job {}: {}",
        job.job_id,
        head_chars(&job.command, 60)
    ));
    write_heartbeat(store, &job.job_id).await?;
    let hb_task = start_heartbeat_task(store.clone(), job.job_id.clone(), pid);
    let slot = Slot {
        job,
        pid: Some(pid),
        peak_vram_gb: 0,
    };
    Ok(Some(ActiveSlot {
        slot,
        child,
        log_file: Some(log_file),
        last_hb: Instant::now(),
        workdir_missing: false,
        paused: false,
        started_mono: Instant::now(),
        _hb_task: hb_task,
        disk_cleanup_lock: None,
        gpu_uuid: gpu_uuid.map(str::to_string),
    }))
}
