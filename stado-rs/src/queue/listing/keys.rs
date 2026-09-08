//! The index name: how a job maps to its marker key, and how a marker is
//! told apart from the other objects sharing the prefix.

use crate::models::Job;

use super::MARKER_PREFIX;

/// Sortable name component: lower = higher real priority + older.
///
/// Python `priority_key`: priority is clamped to 0..=99999999, inverted,
/// and zero-padded to 8 digits, followed by the ISO created_at.
pub fn priority_key(job: &Job) -> String {
    let prio = job.priority.clamp(0, 99_999_999);
    let inv = 99_999_999 - prio;
    format!("{inv:>08}-{}", job.created_at)
}

/// The marker name for `job`.
///
/// Deriving it from the job is what makes marker removal a single delete.
/// While it was only ever recovered by walking the index for a matching
/// suffix, every removal cost a listing of the whole index — tolerable while
/// the index held just the priority>0 jobs, a per-completion scan of the
/// entire queue now that it holds all of them.
pub fn marker_path(job: &Job) -> String {
    format!("{MARKER_PREFIX}{}-{}.json", priority_key(job), job.job_id)
}

/// Whether this path is a marker rather than the migration sentinel (or any
/// other bookkeeping object that shares the prefix).
///
/// Deliberately NOT a job_id parse. The name is
/// `<inv_priority>-<created_at>-<job_id>.json` and BOTH of the trailing
/// fields contain `-` of their own: `created_at` carries the date separators
/// (and a negative UTC offset would carry another), and a job_id looks like
/// `job-906b84bcaf55e7935aa9ba2d`. So there is no split position derivable
/// from the name alone — taking the segment after the last `-` yields
/// `906b84bcaf55e7935aa9ba2d`, an id that resolves to nothing. The marker
/// body states the job_id, and that is what readers use.
pub fn is_marker(path: &str) -> bool {
    path.rsplit('/')
        .next()
        .is_some_and(|name| !name.starts_with('.') && name.ends_with(".json"))
}
