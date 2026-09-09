//! Durable run manifests are the admission and recovery authority above jobs.
//!
//! One submission request owns one `runs/<run_id>.json` document. Its `entries`
//! are CAS-mutated through planned/claimed/enqueuing/accepted/terminal/reaped;
//! terminal entries retain the final job document so deleting lifecycle blobs
//! never turns an old request into new queue work.
//!
//! `prefixes` names the blob prefix those documents live under and the
//! lifecycle prefixes a member job can sit in; `manifest` reads them and
//! derives a run's per-state counts; `terminal` retains a settled job in the
//! entry that names it; `name` derives a run's readable name at submission.

mod manifest;
mod name;
mod prefixes;
mod terminal;

pub use manifest::{list_runs, read_run, run_status, RunStatus};
pub use name::derive_run_name;
pub use prefixes::{ALL_PREFIXES, RUN_PREFIX, TERMINAL_PREFIXES};
pub use terminal::record_terminal_outcome;

pub(crate) use terminal::{record_terminal_outcome_for_entry, terminal_job_matches_entry};
