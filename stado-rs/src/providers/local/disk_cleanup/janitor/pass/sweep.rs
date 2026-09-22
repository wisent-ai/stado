//! The sweep itself, once a pass has decided it is going to run.
//!
//! Split out of `janitor/pass/mod.rs`, which had grown past the module line
//! cap; the lock, the interval gate and the outcome stay there.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::providers::local::disk_cleanup::janitor::pass::cleaners::run_cleaners;
use crate::providers::local::disk_cleanup::janitor::pass::cleaners::summary::summarize_scan;
use crate::providers::local::disk_cleanup::janitor::policy::roots::free_bytes;
use crate::providers::local::disk_cleanup::janitor::state::report::CleanupReport;
use crate::targets::DiskCleanupPolicy;

/// Run every declared cleaner and measure what the volume has left.
///
/// `None` means a step failed and has already written its error and outcome
/// into the report, so the caller finishes the pass the same way in both
/// cases.
pub(super) async fn sweep(
    home: &Path,
    policy: &DiskCleanupPolicy,
    declared_release_versions: &BTreeMap<String, BTreeSet<String>>,
    attempted_at: f64,
    report: &mut CleanupReport,
) -> Option<i64> {
    if let Err(exc) = run_cleaners(
        home,
        policy,
        declared_release_versions,
        attempted_at,
        report,
    )
    .await
    {
        report.add_error("runtime", &exc);
        report.outcome = "invalid_or_unavailable_policy".to_string();
        return None;
    }
    // The store this host serves for the fleet's products. It belongs here
    // rather than to whichever product wrote the bytes, because a host under
    // disk pressure refuses jobs — including the job that would have run
    // that product's own retention — so reclamation owned by the queue never
    // reaches the host that needs it most.
    crate::providers::local::disk_cleanup::object_evidence::scan_object_evidence(
        home,
        policy,
        attempted_at,
        policy.max_scan_items,
        std::time::Instant::now()
            + std::time::Duration::from_secs(policy.max_pass_seconds.unwrap_or(600).max(1) as u64),
        policy.mode == "enforce",
        report,
    );
    // After the cleaners and before the volume is measured: on a Mac their
    // deletions are worth nothing until the snapshots pinning those blocks
    // are thinned, so a pass can remove every tagged build tree it finds and
    // leave free space exactly where it found it. Bounded by the declared
    // target, so a host with headroom keeps its backup history, and skipped
    // entirely when the policy does not declare the cleaner.
    crate::providers::local::disk_cleanup::local_snapshots::thin_to_target(
        home,
        policy,
        policy.mode == "enforce",
        report,
    );
    summarize_scan(policy, report);
    match free_bytes(home) {
        Ok(free) => Some(free),
        Err(exc) => {
            report.add_error("runtime", &exc);
            report.outcome = "invalid_or_unavailable_policy".to_string();
            None
        }
    }
}
