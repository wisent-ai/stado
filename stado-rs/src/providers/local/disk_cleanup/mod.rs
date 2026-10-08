//! The disk-full rule for local compute hosts.
//!
//! Port of `stado/providers/local/disk/cleanup.py` (the "janitor"). Only
//! fixed roots and fixed cleaner implementations exist here, and the only
//! instruction is [`rule`]: when the volume holding the agent's home is at
//! least [`rule::DISK_FULL_PERCENT`] used, a pass deletes everything the fleet
//! put on the host. The registry supplies no path, command, limit or
//! retention.
//!
//! Layout: [`safefs`] holds the dir_fd-relative primitives (and the only
//! `unsafe`), [`hf`] the HuggingFace cache eviction, [`weles`] the weles
//! recordings cleanup, [`build_caches`] the eviction of directories a build
//! tool tagged as regenerable, [`chromium_clones`] the eviction of the bundle
//! clones macOS makes to validate Chromium's signature at every launch. This
//! module owns the report model, the sanitized public state, the
//! exclusive/shared lock file and the top-level [`run_cleanup_once`]
//! orchestration.

pub mod agent_logs;
pub mod backup_twins;
pub mod build_caches;
pub mod chromium_clones;
pub mod compiler_cache;
pub mod consent;
pub mod delivered_releases;
pub mod hf;
mod janitor;
pub mod job_outputs;
pub mod local_snapshots;
pub mod object_evidence;
pub mod queue_workdirs;
pub mod release_store;
pub mod rule;
pub mod safefs;
pub mod weles;

pub use janitor::pass::cleaners::budget::ScanCount;
pub use janitor::pass::lock::workload::{
    acquire_workload_lock, acquire_workload_lock_in, release_workload_lock, WorkloadLock,
    CLEANUP_IN_PROGRESS, CLEANUP_LOCK_ERROR,
};
pub use janitor::pass::lock::{ensure_state_dir, secure_home};
pub use janitor::pass::once::entry::{preview_cleanup_once, run_cleanup_once, CleanupWriter};
pub use janitor::pass::once::keep_list::live_job_ids_within;
pub use janitor::policy::resolve_target;
pub use janitor::policy::roots::{fixed_root, free_bytes};
pub use janitor::state::error::JanitorError;
pub use janitor::state::report::canonical::canonical_json;
pub use janitor::state::report::sanitize::{
    read_cleanup_state, read_cleanup_state_in, sanitize_cleanup_report, sanitize_report,
};
pub use janitor::state::report::{CleanerReport, CleanupReport};
pub use janitor::{lock_relative_path, state_relative_path, STATE_VERSION};

pub(crate) use janitor::pass::lock::euid;
pub(crate) use janitor::pass::lock::file::lock_contended;
pub(crate) use janitor::state::promise::{
    pass_was_prevented as janitor_pass_was_prevented, PROMISES as JANITOR_PROMISES,
};
pub(crate) use janitor::{ifmt, GIB, IFDIR, IFLNK, IFREG, STATE_DIR_PARTS};
