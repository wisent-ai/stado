//! The submit options a rerun replays, field by field.

use crate::models::Job;
use crate::queue::submit::{stable_run_id, ResolvedHardwareProjection, SubmitOptions};

/// The [`SubmitOptions`] that reproduce `original`'s spec.
///
/// Every routing field is pinned to what the original RESOLVED to rather
/// than to the flags that produced it — the job document records the former
/// and never the latter — so the rerun lands on the same hardware. That is
/// the whole content of "resubmit with identical spec".
///
/// Resolved hardware is carried through the explicit replay projection. This
/// bypasses current sizing catalogs, including for the CPU-default marker,
/// rather than inferring hardware again during the rerun.
///
/// `re_submission_of` is set to the id being rerun. That is what makes
/// [`crate::queue::tombstone::on_transition`] write `fixed/<id>.json` or
/// `failed_again/<id>.json` when this job terminates, so "is the original
/// fixed?" stays a list diff instead of a rescan. `schedule_id` is
/// deliberately NOT carried: a manual rerun is not a scheduled submission,
/// and claiming otherwise would corrupt the schedule's own accounting.
pub(super) fn rerun_options(original: &Job, retry_token: &str) -> SubmitOptions {
    SubmitOptions {
        provider: original.provider.clone(),
        // The caller retains retry_token, so a crash retries this manifest
        // rather than opening a second batch.
        batch_id: stable_run_id(
            "rerun-batch",
            &format!("{}\0{retry_token}", original.job_id),
        ),
        run_id: stable_run_id("rerun", &format!("{}\0{retry_token}", original.job_id)),
        bucket: crate::config::bucket().to_string(),
        preemptible: original.preemptible,
        max_cost_per_hour_usd: original.max_cost_per_hour_usd,
        pin_to_provider: original.pin_to_provider,
        priority: original.priority,
        deadline_at: original.deadline_at.clone(),
        repo: original.repo.clone(),
        repo_ref: original.repo_ref.clone(),
        repo_workdir: original.repo_workdir.clone(),
        repo_extras: original.repo_extras.clone(),
        resolved_hardware: Some(ResolvedHardwareProjection {
            gpu_mem_gb: original.gpu_mem_gb,
            gpu_type: original.gpu_type.clone(),
            machine_type: original.machine_type.clone(),
        }),
        pre_command: original.pre_command.clone(),
        apt_packages: original.apt_packages.clone(),
        output_uri: original.output_uri.clone(),
        verify_command: original.verify_command.clone(),
        exclusive: original.exclusive,
        re_submission_of: original.job_id.clone(),
        yieldable: original.yieldable,
        yield_command: original.yield_command.clone(),
        yield_grace_seconds: original.yield_grace_seconds,
        pinned_host: original.pinned_host.clone(),
        platform_os: original.platform_os.clone(),
        architecture: original.architecture.clone(),
        secret_env: original.secret_env.clone(),
        // The resolved map is the reproducible half: aliases were already
        // pinned to immutable versions at the original submit, and a rerun
        // must read the same bytes the original read.
        input_artifacts: original.input_artifacts.clone(),
        resolved_input_artifacts: original.resolved_input_artifacts.clone(),
        ..SubmitOptions::default()
    }
}
