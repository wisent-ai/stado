//! The fixed cleaner order and one pass's run of every cleaner.
//!
//! The rebuildable-cache cleaners run here; the store-backed ones — job work
//! trees, job outputs, replica twins, release versions — run in [`store`].
//! Time Machine's local snapshots go last, because what deleting them
//! recovers is the blocks every other cleaner's deletions left pinned.

pub(crate) mod budget;
mod store;
pub(crate) mod summary;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::providers::local::disk_cleanup::janitor::state::error::JanitorError;
use crate::providers::local::disk_cleanup::janitor::state::report::CleanupReport;
use crate::providers::local::disk_cleanup::{
    agent_logs, build_caches, chromium_clones, hf, local_snapshots, object_evidence, weles,
};

/// What one pass needs from outside the host's own filesystem.
pub(crate) struct PassInputs<'a> {
    /// Whether candidates are removed or only counted.
    pub enforcing: bool,
    /// Every release version the registry declares, by product. `None` when
    /// the registry could not be read; the release store then keeps
    /// everything, because it cannot know which versions other hosts run.
    pub declared_release_versions: Option<&'a BTreeMap<String, BTreeSet<String>>>,
    /// The host's declared Weles recordings directory, when it has one.
    pub weles_recordings_dir: Option<&'a str>,
}

/// Run every cleaner, in order.
///
/// `Err` carries exactly the error the HuggingFace scan escaped with, which
/// the caller records as `runtime`.
pub(crate) async fn run_cleaners(
    home: &Path,
    inputs: &PassInputs<'_>,
    report: &mut CleanupReport,
) -> Result<(), JanitorError> {
    let enforcing = inputs.enforcing;
    // Past every early return: from here the cleaner table is a measurement
    // this pass actually made, so the report may carry one.
    report.scanned = true;
    hf::run_hf(home, enforcing, report.active_job_count, report)?;
    weles::scan_weles(home, inputs.weles_recordings_dir, enforcing, report);
    build_caches::scan_build_caches(home, enforcing, report);
    // The only cleaner whose root is outside this account's home: macOS puts
    // the clones in the per-user temporary container.
    chromium_clones::scan_chromium_clones(home, enforcing, report);
    store::run_store_cleaners(home, inputs, report).await;
    // Product run evidence in the store this host serves. It belongs here
    // rather than to whichever product wrote the bytes, because a full host
    // refuses jobs — including the job that would have run that product's
    // own retention.
    object_evidence::scan_object_evidence(home, enforcing, report);
    agent_logs::scan_agent_logs(home, enforcing, report);
    local_snapshots::delete_all(home, enforcing, report);
    Ok(())
}
