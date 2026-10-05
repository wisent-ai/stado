//! stado — job queue and compute management for Wisent GPU workloads.
//!
//! Rust port of the Python `stado` package (v0.4.388). The on-storage JSON
//! schema is byte-compatible with the Python implementation: job state is
//! encoded in blob prefixes (`queue/`, `running/`, `completed/`, `uploaded/`,
//! `failed/`, `cancelled/`) and blobs are `Job` JSON documents.

pub mod artifacts;
pub mod artifacts_models;
pub mod autonomy;
pub mod binary;
pub mod capabilities;
pub mod catalog;
pub mod cli;
pub mod config;
pub mod config_file;
pub mod coordinator;
pub mod credential_store;
pub mod dashboard;
pub mod declaration;
pub mod deploy;
pub mod doctor;
pub mod failure_fixer;
pub mod fleet_expansion;
pub mod fleet_needs;
pub mod fleet_shape;
pub mod github_identity;
pub mod host_software;
pub mod inference;
pub mod machine;
pub mod mail;
pub mod mcp;
pub mod models;
pub mod monitor;
pub mod observations;
pub mod placement;
pub mod primitives;
pub mod profiles;
pub mod providers;
pub mod public_origin;
pub mod queue;
pub mod rate_limit;
pub mod registry_import;
pub mod release_agent;
pub mod release_cause;
pub mod release_control;
pub mod release_pipeline;
pub mod remote;
// Crate-private: the revisit pass has exactly two callers, the release agent's
// tick and `registry doctor`'s annotation, both inside this crate. Nothing
// outside it should be able to reach a function that restarts a launchd unit.
pub(crate) mod release_unit_image;
pub mod scheduler;
pub mod schedules;
pub mod self_update;
pub mod service_resolution;
pub mod sizing;
pub mod skarbiec;
pub mod stream;
pub mod targets;
pub mod transcripts;
pub mod watchdog;
