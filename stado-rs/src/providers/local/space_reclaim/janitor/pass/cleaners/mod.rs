//! The fixed cleaner order and one pass's run of every cleaner.
//!
//! The rebuildable-cache cleaners run here; the store-backed ones — job work
//! trees, job outputs, replica twins, release versions — run in [`store`].
//! Time Machine's local snapshots go last, because what deleting them
//! recovers is the blocks every other cleaner's deletions left pinned. The
//! build-cache walk is not here: it reads the whole home, so it runs before
//! the pass takes the exclusive cleanup lock and takes that lock only for
//! each cache it removes ([`once`](super::once)). Neither is the queue: the
//! store cleaners act on answers asked before the lock ([`asked`]).

pub(crate) mod asked;
pub(crate) mod budget;
mod store;
pub(crate) mod summary;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::providers::local::space_reclaim::janitor::policy::ownership;
use crate::providers::local::space_reclaim::janitor::state::error::JanitorError;
use crate::providers::local::space_reclaim::janitor::state::report::CleanupReport;
use crate::providers::local::space_reclaim::{
    agent_logs, chromium_clones, compiler_cache, delivered_releases, hf, local_snapshots,
    object_evidence, weles,
};

/// What one pass needs from outside the host's own filesystem.
pub(crate) struct PassInputs<'a> {
    /// Whether candidates are removed or only counted.
    pub enforcing: bool,
    /// The one tree a pass may delete in, `None` for the whole host: on a
    /// shared host, Stado's own, outside which candidates are only counted.
    pub deletable: Option<&'a Path>,
    /// Every release version the registry declares, by product. `None` when
    /// the registry could not be read; the release store then keeps
    /// everything, because it cannot know which versions other hosts run.
    pub declared_release_versions: Option<&'a BTreeMap<String, BTreeSet<String>>>,
    /// The host's declared Weles recordings directory, when it has one.
    pub weles_recordings_dir: Option<&'a str>,
    /// The queue's answers about the job work trees and outputs on disk,
    /// asked before the exclusive lock.
    pub asked: &'a asked::Asked,
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
    // A cleaner whose root lies outside the tree this host lets Stado delete
    // in (a shared host's model downloads, compiler cache, harness logs,
    // browser clones and Time Machine snapshots) measures and reports its
    // candidates, removes none, and says why.
    let foreign = enforcing && inputs.deletable.is_none();
    let kept = enforcing && inputs.deletable.is_some();
    // Past every early return: from here the cleaner table is a measurement
    // this pass actually made, so the report may carry one.
    report.scanned = true;
    hf::run_hf(home, foreign, report.active_job_count, report)?;
    let weles_owned = inputs
        .weles_recordings_dir
        .is_some_and(|dir| ownership::may_delete(inputs.deletable, &home.join(dir)));
    weles::scan_weles(
        home,
        inputs.weles_recordings_dir,
        enforcing && (inputs.deletable.is_none() || weles_owned),
        report,
    );
    // The only cleaner whose root is outside this account's home: macOS puts
    // the clones in the per-user temporary container.
    chromium_clones::scan_chromium_clones(home, foreign, report);
    store::run_store_cleaners(home, inputs, report).await;
    // Product run evidence in the store this host serves. It belongs here
    // rather than to whichever product wrote the bytes, because a full host
    // refuses jobs — including the job that would have run that product's
    // own retention.
    object_evidence::scan_object_evidence(home, enforcing, report);
    // The copies release delivery stages for attestation; the installed and
    // newest version of each product stay.
    delivered_releases::scan_delivered_releases(home, enforcing, report);
    // The compiler cache's store; kept while a job runs, since a build reads
    // and writes it.
    compiler_cache::scan_compiler_cache(home, foreign, report);
    agent_logs::scan_agent_logs(home, foreign, report);
    local_snapshots::delete_all(home, foreign, report);
    if kept {
        let mut weles_kept = (!weles_owned).then_some(&mut report.weles);
        for cleaner in [
            Some(&mut report.hf),
            weles_kept.take(),
            Some(&mut report.clones),
            Some(&mut report.compiler_cache),
            Some(&mut report.agent_logs),
            Some(&mut report.local_snapshots),
        ]
        .into_iter()
        .flatten()
        {
            *cleaner
                .skipped
                .entry(ownership::NOT_STADO_OWNED.to_string())
                .or_default() += 1;
        }
    }
    Ok(())
}
