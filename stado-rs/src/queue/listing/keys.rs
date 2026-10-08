//! The index name: how a job maps to its marker key, and how a marker is
//! told apart from the other objects sharing the prefix.

use crate::models::Job;

use super::MARKER_PREFIX;

/// The highest priority a marker key can order: the key spells the inverted
/// priority in eight zero-padded digits, so a ninth digit would sort wrong.
/// A submission or a priority change past it is refused, never clamped.
pub const PRIORITY_LIMIT: i64 = 99_999_999;

/// Sortable name component: lower = higher real priority + older.
///
/// Python `priority_key`: priority inverted against [`PRIORITY_LIMIT`] and
/// zero-padded to eight digits, followed by the ISO created_at. Submission
/// and `job priority` refuse a priority that does not fit
/// ([`priority_fits`]); a record an older writer stored outside it is
/// ordered at the nearer end.
pub fn priority_key(job: &Job) -> String {
    let prio = job.priority.clamp(0, PRIORITY_LIMIT);
    let inv = PRIORITY_LIMIT - prio;
    format!("{inv:>08}-{}", job.created_at)
}

/// Whether `priority` fits the marker key: not below zero, not past
/// [`PRIORITY_LIMIT`].
pub fn priority_fits(priority: i64) -> bool {
    !priority.is_negative() && priority <= PRIORITY_LIMIT
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
