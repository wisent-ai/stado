//! Retention of a settled member job in the manifest entry that names it.
//!
//! `projection` proves a terminal job is exactly the immutable plan its entry
//! holds; `record` is the CAS loop that pins the outcome into the manifest.

mod projection;
mod record;

pub use record::record_terminal_outcome;

pub(crate) use projection::terminal_job_matches_entry;
pub(crate) use record::record_terminal_outcome_for_entry;
