//! Generic job-completion coverage verifier + retry orchestrator.
//!
//! Port of `stado/coverage/__init__.py`, `stado/coverage/failures.py` (the
//! [`failures`] submodule), and `stado/coverage/cli.py` ([`cli_main`]).
//!
//! Universe-agnostic: nothing here knows about activation extraction,
//! training, eval, or any specific job type. A Universe yields
//! [`UniverseEntry`] tuples (group_key, command, expected_uri) and supplies
//! a [`Verifier`]. The orchestrator walks the universe, checks each
//! expected output, diffs against state, re-submits the gap subset via
//! `queue::submit`, and tracks per-group_key attempts at
//! `<COVERAGE_STATE_PREFIX>/<universe_id>/state.json`. After
//! `COVERAGE_ATTEMPT_CAP` attempts a group_key is UNFIXABLE and surfaced
//! but not re-submitted.
//!
//! DEVIATION: Python discovers Universe classes via importlib.metadata
//! entry_points group `stado.coverage_universes`. Rust has no entry-point
//! analog, so discovery is a static in-process registry: downstream crates
//! call [`register_universe`] at startup with a factory. The `stado` crate
//! itself ships no universes (they are external plugins in Python too), so
//! the registry is empty by default and every universe id is "unknown".
//! The unknown-universe error message is byte-identical to Python's.

mod cli;
mod orchestrator;
mod universe;

use crate::queue::StorageError;

pub use cli::{cli_main, coerce, kv_to_kwargs};
pub use orchestrator::{
    retry_gaps, state_load, state_save, verify, verify_and_retry, verify_and_retry_with_store,
    CoverageReport,
};
pub use universe::{
    build_universe, list_universes, register_universe, registered_universe_names,
    unknown_universe_message, StadoObjectExistsVerifier, URIExistsVerifier, Universe,
    UniverseEntry, UniverseFactory, Verifier,
};

/// Python `PRESENT` — verifier outcome: the expected output exists.
pub const PRESENT: &str = "present";
/// Python `MISSING` — verifier outcome: the expected output is absent.
pub const MISSING: &str = "missing";
/// Python `UNFIXABLE` — group_key exhausted COVERAGE_ATTEMPT_CAP attempts.
pub const UNFIXABLE: &str = "unfixable";

/// Coverage-layer error. Python raises `ValueError` (verifier misuse),
/// `RuntimeError` (HTTP retry-cap), and lets urllib/storage/submit
/// exceptions propagate; the variants here cover those.
#[derive(Debug, thiserror::Error)]
pub enum CoverageError {
    #[error("{0}")]
    Other(String),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Submit(#[from] crate::queue::submit::SubmitError),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl From<&str> for CoverageError {
    fn from(msg: &str) -> Self {
        Self::Other(msg.to_string())
    }
}

impl From<String> for CoverageError {
    fn from(msg: String) -> Self {
        Self::Other(msg)
    }
}

// ---------------------------------------------------------------------------
// failures.py — bridge from failed/ blob store -> coverage state
// ---------------------------------------------------------------------------

/// Port of `stado/coverage/failures.py`. The Job model does not (yet)
/// carry a `coverage_universe_id` / `coverage_group_key` field, so
/// failures cannot be propagated to the universe state file from inside
/// the coordinator's running -> failed transition. Until that field
/// lands, [`failures::scan_failed_commands`] provides the back-reference.
pub mod failures;
