//! Job data model and state definitions.
//!
//! Port of `stado/models.py`. The JSON representation is byte-compatible with
//! the Python `Job.to_json()` (`json.dumps(asdict(job), indent=2)` with
//! `ensure_ascii=True`), including field declaration order. `from_dict`
//! tolerance maps to serde: unknown keys are ignored, missing keys resolve
//! to the Python dataclass defaults via `#[serde(default = ...)]`.

mod activation;
mod job;
mod python_compat;
mod states;

pub use activation::{removed_activation_command_reason, REMOVED_ACTIVATION_PACKAGE};
pub use job::{Job, JobSecretRef, WorkerAllocation, WorkerResource};
pub use states::job_state;

pub(crate) use python_compat::{
    ensure_ascii, isoformat_utc, json_dumps_pretty_sorted, py_str_repr,
};
