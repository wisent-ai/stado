//! Placement and routing: the CPU marker SKU, the sizing scan behind it, the
//! per-command hardware resolution and the planned job every phase after this
//! one persists.

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
/// submit-time bucket option), using the process-wide sizing caches; `None`
/// when the command names no model, decided without constructing the
/// storage handle, like Python returning before the sizing import does any
/// GCS work.
async fn estimate_gpu_mem(command: &str) -> Result<Option<i64>, SubmitError> {
    if crate::sizing::model_of(command).is_none() {
        return Ok(None);
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
    // The caller's `--vram-gb` when it states one; otherwise the size the
    // command's model measured. Neither is a job that sizes to nothing.
    let stated_vram = options.vram_gb.is_positive().then_some(options.vram_gb);
    let caller_asked_for_gpu =
        !options.gpu_type.is_empty() || stated_vram.is_some() || !options.machine_type.is_empty();
    let mut gpu_mem = match stated_vram {
        Some(stated) => Some(stated),
        None => estimate_gpu_mem(command).await?,
    };
    let sizing = GPU_SIZING.get(options.provider.as_str());
    let (machine_type, gpu_type) = if !caller_asked_for_gpu && gpu_mem.is_none() {
        (CPU_MACHINE_TYPE.into(), String::new())
    } else {
        // The provider's tier for the size, its smallest tier for a job of
        // no known size, and no tier for a provider that sizes nothing.
        let inferred = match gpu_mem {
            Some(mem) => config::lookup_instance_type(&options.provider, mem),
            None => sizing.and_then(|tiers| tiers.values().next().copied()),
        };
        let (inferred_machine, inferred_accel) = match inferred {
            Some((machine, accel)) => (machine.to_string(), accel.to_string()),
            None => (String::new(), String::new()),
        };
        let gpu_type = if options.gpu_type.is_empty() {
            inferred_accel
        } else {
            options.gpu_type.clone()
        };
        let machine_type = if !options.machine_type.is_empty() {
            options.machine_type.clone()
        } else if !options.gpu_type.is_empty() && stated_vram.is_none() {
            let tier = sizing.and_then(|tiers| {
                tiers
                    .iter()
                    .find(|(_, (_, accel))| *accel == options.gpu_type)
            });
            match tier {
                Some((mem, (machine, _))) => {
                    if gpu_mem.is_none() {
                        gpu_mem = Some(*mem);
                    }
                    machine.to_string()
                }
                None => inferred_machine,
            }
        } else {
            inferred_machine
        };
        (machine_type, gpu_type)
    };
    // A job of no known size records the caller's `--vram-gb`, which stated
    // none.
    let gpu_mem_gb = match gpu_mem {
        Some(mem) => mem,
        None => options.vram_gb,
    };
    Ok(ResolvedHardwareProjection {
        gpu_mem_gb,
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
    job.cpu_cores = options.cpu_cores;
    job.memory_gb = options.memory_gb;
    job.provider = options.provider.clone();
    job.batch_id = options.batch_id.clone();
    job.preemptible = options.preemptible;
    job.max_cost_per_hour_usd = options.max_cost_per_hour_usd;
    job.submitter_restarts = options.submitter_restarts;
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
    job.terminates_agent = options.terminates_agent;
    job.schedule_id = options.schedule_id.clone();
    job.re_submission_of = options.re_submission_of.clone();
    job.yieldable = options.yieldable;
    job.yield_command = options.yield_command.clone();
    if let Some(max_yields) = options.max_yields_before_protected {
        job.max_yields_before_protected = max_yields;
    }
    if let Some(boot_disk_gb) = options.boot_disk_gb {
        job.boot_disk_gb = boot_disk_gb;
    }
    if let Some(runtime_seconds_estimate) = options.runtime_seconds_estimate {
        job.runtime_seconds_estimate = runtime_seconds_estimate;
    }
    job.pinned_host = options.pinned_host.clone();
    job.secret_env = options.secret_env.clone();
    job.input_artifacts = options.input_artifacts.clone();
    job.resolved_input_artifacts = options.resolved_input_artifacts.clone();
    job
}
