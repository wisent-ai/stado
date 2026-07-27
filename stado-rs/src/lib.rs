//! stado — job queue and compute management for Wisent GPU workloads.
//!
//! Rust port of the Python `stado` package (v0.4.388). The on-storage JSON
//! schema is byte-compatible with the Python implementation: job state is
//! encoded in blob prefixes (`queue/`, `running/`, `completed/`, `uploaded/`,
//! `failed/`, `cancelled/`) and blobs are `Job` JSON documents.

pub mod artifacts;
pub mod artifacts_models;
pub mod catalog;
pub mod cli;
pub mod config;
pub mod config_file;
pub mod constants;
pub mod control_plane;
pub mod coordinator;
pub mod coverage;
pub mod dashboard;
pub mod deploy;
pub mod doctor;
pub mod failure_fixer;
pub mod machine;
pub mod mail;
pub mod mcp;
pub mod models;
pub mod monitor;
pub mod profiles;
pub mod providers;
pub mod queue;
pub mod scheduler;
pub mod schedules;
pub mod self_update;
pub mod sizing;
pub mod targets;
pub mod watchdog;

pub(crate) mod azure_token;
pub(crate) mod azure_key_vault;
pub(crate) mod procutil;

#[cfg(test)]
mod testutil;

/// Root of the crate's bundled data directory: `data/` in the source tree,
/// holding the byte-identical copies of the Python package data (profiles,
/// startup-script templates, the compute-target registry).
///
/// `CARGO_MANIFEST_DIR` is frozen at compile time, so this path resolves
/// only on the machine that built the binary. Anything a shipped `stado`
/// must read belongs in `include_str!` instead — see
/// [`targets::load_bundled_registry`] and the agent startup templates in
/// [`scheduler::dispatch::agent`]. The remaining callers are all
/// build-tree-local: profile discovery, submit-time job templates, and the
/// operator-facing data-dir path print.
pub fn data_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("data")
}
