//! Prove a settled job is exactly the immutable plan its manifest entry holds.

use serde_json::Value;

fn terminal_job_projection(job: &crate::models::Job) -> Value {
    let mut projection = crate::queue::submit::immutable_job_projection(job);
    let object = projection
        .as_object_mut()
        .expect("Job projection serializes as an object");
    for field in [
        "run_id",
        "submission_request_digest",
        "submission_command_index",
    ] {
        object.remove(field);
    }
    projection
}

pub(crate) fn terminal_job_matches_entry(
    job: &crate::models::Job,
    planned: &crate::models::Job,
    run_id: &str,
    index: usize,
) -> bool {
    let exact_linkage = job.run_id == run_id
        && job.submission_request_digest == planned.submission_request_digest
        && job.submission_command_index == Some(index);
    let legacy_unlinked = job.run_id.is_empty()
        && job.submission_request_digest.is_empty()
        && job.submission_command_index.is_none();
    planned.run_id == run_id
        && planned.submission_command_index == Some(index)
        && job.job_id == planned.job_id
        && (exact_linkage || legacy_unlinked)
        && terminal_job_projection(job) == terminal_job_projection(planned)
}
