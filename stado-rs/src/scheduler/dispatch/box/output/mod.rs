//! Bounded command, prompt, and artifact output helpers.
//!
//! Port of `stado/scheduler/dispatch/box/output.py`, plus the two helpers
//! it imports from `stado/providers/local/helpers/execution.py`
//! (`build_job_command`, `verify_command` — "Pure shell command assembly
//! shared by local and structured providers").
//!
//! Deviation: Python `upload_artifacts` requires an SDK storage backend
//! (`store._blob_backend` / `store._sdk_bucket`) and raises RuntimeError
//! otherwise; every Rust `BlobBackend` uploads bytes by contract, so the
//! gate has no analog and artifacts always land via
//! [`crate::queue::JobStorage::upload_bytes`].
//!
//! The helpers are split along the seams the port already had: `shell`
//! assembles the job command out of its preludes, `wrapper` lays out the
//! on-box runtime and the run.sh that drives it, `responses` reads file and
//! event responses back, and `artifacts` collects the bounded artifact set.
//! The bounds every one of them reads stay here, above them.

mod artifacts;
mod responses;
mod shell;
mod wrapper;

pub const LOG_BYTES: usize = 57344;
pub const ARTIFACT_BYTES: usize = 16777216;
pub const ARTIFACT_COUNT: usize = 16;
pub const EVENT_PAGES: usize = 10;
pub const EVENT_LIMIT: i64 = 100;

/// Called out of tree by `super::runtime`'s terminal pass, which collects a
/// finished workload's artifacts before it releases the lease.
pub(crate) use artifacts::upload_artifacts;
/// Called out of tree by `super::runtime`'s reconcile pass, which unwraps the
/// stdout, stderr and exit-code file responses it reads back.
pub use responses::file_content;
/// `prompt_output` is called out of tree by `super::runtime`'s reconcile pass
/// and `recover_prompt_id` by its start pass.
pub(crate) use responses::{prompt_output, recover_prompt_id};
/// Called out of tree by `super::runtime`'s start and control passes, which
/// quote the runtime paths and the operation id they interpolate into the
/// commands they run in the box.
pub(crate) use shell::shell_quote;
/// Re-exported out of tree by `crate::providers::local`, which serves both to
/// its slot passes at their published `providers::local::` path; they also
/// keep their own published `dispatch::box::output::` path here.
pub use shell::{build_job_command, verify_command};
/// `runtime_paths` is called out of tree by `super::runtime`'s start, control
/// and reconcile passes, and `command_wrapper` by its start pass;
/// `RuntimePaths` is what `runtime_paths` returns and what `command_wrapper`
/// takes, so it stays nameable beside them.
pub use wrapper::{command_wrapper, runtime_paths, RuntimePaths};
