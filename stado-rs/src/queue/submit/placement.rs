//! Placement and routing: the CPU marker SKU, the sizing scan behind it, the
//! per-command hardware resolution and the planned job every phase after this
//! one persists.

use std::collections::BTreeMap;

use crate::catalog::GPU_SIZING;
use crate::config;
use crate::models::Job;

use super::{
    default_store, ResolvedHardwareProjection, SubmissionProvenance, SubmitError, SubmitOptions,
};

/// The SKU the CPU branch of [`build_job`] writes: no accelerator was
/// asked for and the command sized to nothing. Named because it is also a
/// *readback* marker — `cli::job` recognizes a job that came out of that
/// branch by this machine_type, and must then resubmit with the routing
/// flags left empty rather than pinning them (pinning any of them flips
/// `caller_asked_for_gpu` and stamps an accelerator onto a CPU job).
///
/// It is a GCE name, kept because it is that readback marker, and it is
/// therefore **not** a portable VM size. `scheduler::dispatch::agent` refuses
/// to hand a machine type to a provider that does not name sizes that way, so
/// this marker cannot reach Azure as `hardwareProfile.vmSize`.
pub const CPU_MACHINE_TYPE: &str = "e2-standard-8";

/// config::estimate_gpu_memory against the configured queue bucket
/// (Python's sizing scan always targets the global BUCKET, not the
/// submit-time bucket option), using the process-wide sizing caches. A
/// regex miss short-circuits to 0 without constructing the storage
/// handle, like Python returning before the sizing import does any GCS
/// work.
async fn estimate_gpu_mem(command: &str) -> Result<i64, SubmitError> {
    if crate::sizing::model_of(command).is_empty() {
        return Ok(0);
    }
    let store = default_store("").await?;
    Ok(config::estimate_gpu_memory(command, crate::sizing::global(), &store).await?)
}

pub(super) async fn resolve_hardware(
    command: &str,
    options: &SubmitOptions,
) -> Result<ResolvedHardwareProjection, SubmitError> {
    if let Some(resolved) = options.resolved_hardware.as_ref() {
        return Ok(resolved.clone());
    }
    let caller_asked_for_gpu =
        !options.gpu_type.is_empty() || options.vram_gb > 0 || !options.machine_type.is_empty();
    let mut gpu_mem = if options.vram_gb > 0 {
        options.vram_gb
    } else {
        estimate_gpu_mem(command).await?
    };
    let (machine_type, gpu_type) = if !caller_asked_for_gpu && gpu_mem == 0 {
        (CPU_MACHINE_TYPE.into(), String::new())
    } else {
        let (inferred_machine, inferred_accel) =
            config::lookup_instance_type(&options.provider, gpu_mem);
        let gpu_type = if options.gpu_type.is_empty() {
            inferred_accel.to_string()
        } else {
            options.gpu_type.clone()
        };
        let machine_type = if !options.machine_type.is_empty() {
            options.machine_type.clone()
        } else if !options.gpu_type.is_empty() && options.vram_gb == 0 {
            let empty = BTreeMap::new();
            let sizing = GPU_SIZING.get(options.provider.as_str()).unwrap_or(&empty);
            match sizing
                .iter()
                .find(|(_, (_, accel))| *accel == options.gpu_type)
            {
                Some((mem, (machine, _))) => {
                    if gpu_mem == 0 {
                        gpu_mem = *mem;
                    }
                    machine.to_string()
                }
                None => inferred_machine.to_string(),
            }
        } else {
            inferred_machine.to_string()
        };
        (machine_type, gpu_type)
    };
    Ok(ResolvedHardwareProjection {
        gpu_mem_gb: gpu_mem,
        gpu_type,
        machine_type,
    })
}

pub(super) fn build_planned_job(
    command: &str,
    options: &SubmitOptions,
    job_id: &str,
    hardware: &ResolvedHardwareProjection,
    provenance: &SubmissionProvenance,
) -> Job {
    let mut job = Job::new(job_id, command);
    job.created_at = provenance.created_at.clone();
    job.gpu_mem_gb = hardware.gpu_mem_gb;
    job.gpu_type = hardware.gpu_type.clone();
    job.machine_type = hardware.machine_type.clone();
    job.platform_os = options.platform_os.clone();
    job.architecture = options.architecture.clone();
    job.provider = options.provider.clone();
    job.batch_id = options.batch_id.clone();
    job.preemptible = options.preemptible;
    job.max_cost_per_hour_usd = options.max_cost_per_hour_usd;
    job.pin_to_provider = options.pin_to_provider;
    job.priority = options.priority;
    job.deadline_at = options.deadline_at.clone();
    job.submitted_by = provenance.submitted_by.clone();
    job.submitted_from = provenance.submitted_from.clone();
    job.submitted_via = "cli".into();
    job.run_id = options.run_id.clone();
    job.submitter_app = provenance.submitter_app.clone();
    job.repo = options.repo.clone();
    job.repo_ref = options.repo_ref.clone();
    job.repo_workdir = options.repo_workdir.clone();
    job.repo_extras = options.repo_extras.clone();
    job.pre_command = options.pre_command.clone();
    job.apt_packages = options.apt_packages.clone();
    job.output_uri = options.output_uri.clone();
    job.verify_command = options.verify_command.clone();
    job.exclusive = options.exclusive;
    job.schedule_id = options.schedule_id.clone();
    job.re_submission_of = options.re_submission_of.clone();
    job.yieldable = options.yieldable;
    job.yield_command = options.yield_command.clone();
    job.yield_grace_seconds = options.yield_grace_seconds;
    job.pinned_host = options.pinned_host.clone();
    job.secret_env = options.secret_env.clone();
    job.input_artifacts = options.input_artifacts.clone();
    job.resolved_input_artifacts = options.resolved_input_artifacts.clone();
    job
}
