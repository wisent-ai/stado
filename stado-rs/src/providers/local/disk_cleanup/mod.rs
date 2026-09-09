//! Registry-authorized, bounded disk cleanup for local compute hosts.
//!
//! Port of `stado/providers/local/disk/cleanup.py` (the "janitor"). Only
//! fixed roots and fixed cleaner implementations exist here. Registry data
//! can select a cleaner and its retention, but can never supply a path or
//! command.
//!
//! Layout: [`safefs`] holds the dir_fd-relative primitives (and the only
//! `unsafe`), [`hf`] the HuggingFace cache eviction, [`weles`] the weles
//! recordings cleanup, [`build_caches`] the eviction of directories a build
//! tool tagged as regenerable, [`chromium_clones`] the eviction of the bundle
//! clones macOS makes to validate Chromium's signature at every launch. This
//! module owns the report model, the sanitized public state, the
//! exclusive/shared lock file, the canonical-policy resolution, and the
//! top-level [`run_cleanup_once`] orchestration.

pub mod backup_twins;
pub mod build_caches;
pub mod catalogue;
pub mod chromium_clones;
pub mod hf;
mod janitor;
pub mod queue_workdirs;
pub mod release_store;
pub mod safefs;
pub mod weles;

pub use janitor::pass::cleaners::budget::ScanBudget;
pub use janitor::pass::lock::workload::{
    acquire_workload_lock, acquire_workload_lock_in, release_workload_lock, WorkloadLock,
};
pub use janitor::pass::lock::{ensure_state_dir, secure_home};
pub use janitor::pass::once::entry::{
    preview_cleanup_once, run_cleanup_once, run_cleanup_to_target_once, CleanupWriter,
};
pub use janitor::pass::once::keep_list::live_job_ids_within;
pub use janitor::policy::resolve_canonical_policy;
pub use janitor::policy::roots::{fixed_root, free_bytes};
pub use janitor::policy::watermarks::{
    disk_pressure_active, disk_pressure_unresolved, persisted_disk_low_bytes,
    persisted_disk_low_bytes_in, validated_report_low_bytes,
};
pub use janitor::state::error::JanitorError;
pub use janitor::state::report::canonical::canonical_json;
pub use janitor::state::report::sanitize::{
    read_cleanup_state, read_cleanup_state_in, sanitize_cleanup_report, sanitize_report,
};
pub use janitor::state::report::{Caps, CleanerReport, CleanupReport};
pub use janitor::{lock_relative_path, state_relative_path, STATE_VERSION};

pub(crate) use janitor::pass::lock::euid;
pub(crate) use janitor::pass::lock::file::lock_contended;
pub(crate) use janitor::{ifmt, GIB, IFDIR, IFLNK, IFREG, STATE_DIR_PARTS};
