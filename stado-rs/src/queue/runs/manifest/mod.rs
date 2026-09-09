//! One `runs/<run_id>.json` document: `read` fetches it and lists every run
//! id under the prefix, `status` derives a run's per-state counts from the
//! prefixes its member jobs currently sit in.

mod read;
mod status;

pub use read::{list_runs, read_run};
pub use status::{run_status, RunStatus};
