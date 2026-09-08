//! The original-versus-rerun comparison table.

use crate::models::Job;

/// The fields a rerun has to reproduce, side by side, so "identical spec"
/// is something the operator can read off the terminal instead of trust.
/// The command is printed above the table rather than in it — it is
/// resubmitted verbatim, and a long one would pad every other row.
pub(super) fn spec_rows(original: &Job, fresh: &Job) -> Vec<Vec<String>> {
    let row = |field: &str, was: String, now: String| vec![field.to_string(), was, now];
    vec![
        row("state", original.state.clone(), fresh.state.clone()),
        row(
            "provider",
            original.provider.clone(),
            fresh.provider.clone(),
        ),
        row(
            "gpu_mem_gb",
            original.gpu_mem_gb.to_string(),
            fresh.gpu_mem_gb.to_string(),
        ),
        row(
            "gpu_type",
            original.gpu_type.clone(),
            fresh.gpu_type.clone(),
        ),
        row(
            "machine_type",
            original.machine_type.clone(),
            fresh.machine_type.clone(),
        ),
        row(
            "priority",
            original.priority.to_string(),
            fresh.priority.to_string(),
        ),
        row(
            "preemptible",
            original.preemptible.to_string(),
            fresh.preemptible.to_string(),
        ),
        row(
            "exclusive",
            original.exclusive.to_string(),
            fresh.exclusive.to_string(),
        ),
        row(
            "pinned_host",
            original.pinned_host.clone(),
            fresh.pinned_host.clone(),
        ),
        row(
            "deadline_at",
            original.deadline_at.clone().unwrap_or_default(),
            fresh.deadline_at.clone().unwrap_or_default(),
        ),
        row(
            "platform_os",
            original.platform_os.clone(),
            fresh.platform_os.clone(),
        ),
        row(
            "architecture",
            original.architecture.clone(),
            fresh.architecture.clone(),
        ),
        row("run_id", original.run_id.clone(), fresh.run_id.clone()),
        row(
            "batch_id",
            original.batch_id.clone(),
            fresh.batch_id.clone(),
        ),
        row(
            "re_submission_of",
            original.re_submission_of.clone(),
            fresh.re_submission_of.clone(),
        ),
    ]
}
