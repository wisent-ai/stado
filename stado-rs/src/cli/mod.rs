//! CLI entry point: submit, status, results, cancel, profiles, config.
//!
//! Port of `stado/cli.py` (click) to clap derive. The full command tree is
//! declared and every branch dispatches to its Rust implementation.
//!
//! Implemented and wired to the library: `package-root`, `capabilities`,
//! `submit`, `status`, `cancel`, `results`, `profiles`, `config`, `schedule`,
//! `artifact`, `cost`, `vast`, `agent`, `disk-cleanup`, `resources`,
//! `install-disk-cleanup`, `bootstrap`, `recovery`, the complete `host`,
//! `registry`, and `quota` groups, plus coordinator and dashboard control planes.
//!
//! The declaration of that tree, the process entry point and the failure type
//! every command answers with live in [`entry`]. This module declares the
//! implementations and re-exports each of those items under the
//! `crate::cli::<name>` path its callers already use.

pub mod artifact;
pub mod azure;
pub mod billing;
pub mod blast_radius;
pub mod builds;
pub mod capabilities;
pub mod cloudflare;
pub mod config_cmd;
pub mod cost;
pub mod dashboard;
pub mod database;
pub mod directory;
pub mod dns;
pub mod doctor;
pub mod entry;
pub mod fleet;
pub mod host;
pub mod hosts;
pub mod identity;
pub mod inference;
pub mod integrations;
pub mod instances;
pub mod job;
pub mod overview;
pub mod placement;
pub mod quota;
pub mod recovery;
pub mod registry;
pub mod reporting;
pub mod release_catalog;
pub mod release_cmd;
pub mod release_evidence;
pub mod release_quarantine;
pub mod release_submit;
pub mod repair;
pub mod resolver;
pub mod resources;
pub mod route;
pub mod runner;
pub mod scratch;
pub mod secrets;
pub mod seed_freshness;
pub mod service;
pub mod setup;
pub mod service_converge;
pub mod service_refresh_image;
pub mod service_verify;
pub mod space;
pub mod storage;
pub mod stream;
pub mod submit;
pub mod web;
pub mod work;
pub mod workdirs;
pub mod workload;

pub use entry::dispatch::main_entry;
pub use entry::error::{http_failure, CmdError, CLICK_ERROR_CODE};
pub use entry::spec::jobs::ScheduleCreateArgs;
pub use entry::spec::Cli;

pub(crate) use entry::spec::fleet::host::users::HostUserCommands;
pub(crate) use entry::spec::fleet::host::HostCommands;
pub(crate) use entry::spec::fleet::identity::IdentityCommands;
pub(crate) use entry::spec::fleet::registry::{
    RegistryCommands, RegistryHostCommands, RegistryHostPathCommands,
};
pub(crate) use entry::spec::jobs::{
    ArtifactAliasCommands, ArtifactCommands, ArtifactImportCommands, MachineCommands,
    ScheduleCommands,
};
pub(crate) use entry::spec::spend::billing::{default_mail_results, BillingCommands, MailCommands};
pub(crate) use entry::spec::spend::cost::CostCommands;
pub(crate) use entry::spec::spend::quota::QuotaCommands;
pub(crate) use entry::spec::spend::vast::VastCommands;
