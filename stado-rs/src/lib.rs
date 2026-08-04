//! stado — job queue and compute management for Wisent GPU workloads.
//!
//! Rust port of the Python `stado` package (v0.4.388). The on-storage JSON
//! schema is byte-compatible with the Python implementation: job state is
//! encoded in blob prefixes (`queue/`, `running/`, `completed/`, `uploaded/`,
//! `failed/`, `cancelled/`) and blobs are `Job` JSON documents.

pub mod artifacts;
pub mod artifacts_models;
pub mod autonomy;
pub mod capabilities;
pub mod catalog;
pub mod cli;
pub mod config;
pub mod config_file;
pub mod constants;
pub mod control_plane;
pub mod coordinator;
pub mod coverage;
pub mod credential_store;
pub mod dashboard;
pub mod deploy;
pub mod doctor;
pub mod failure;
pub mod failure_fixer;
pub mod inference;
pub mod machine;
pub mod mail;
pub mod mcp;
pub mod models;
pub mod monitor;
pub mod object_store;
pub mod placement;
pub mod profiles;
pub mod providers;
pub mod queue;
pub mod rate_limit;
pub mod release;
pub mod scheduler;
pub mod schedules;
pub mod self_update;
pub mod service_resolution;
pub mod sizing;
pub mod skarbiec;
pub mod targets;
pub mod transcripts;
pub mod watchdog;

pub(crate) mod azure_token;
pub(crate) mod procutil;

#[cfg(test)]
mod testutil;

/// Root of the source tree's `data/` directory for build-time tooling.
///
/// `CARGO_MANIFEST_DIR` is frozen at compile time and must never be used to
/// resolve installed runtime assets. Runtime registry, startup templates, and
/// profiles are embedded with `include_str!`; the remaining callers are the
/// operator-facing package-root command and test-only source fixtures.
pub fn data_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("data")
}
