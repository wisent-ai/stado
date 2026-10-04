//! One cleanup pass: the exclusive run lock, the disk-full rule's verdict,
//! the cleaners, and the outcome it reports.

pub(crate) mod cleaners;
pub(crate) mod lock;
pub(crate) mod once;
pub(crate) mod service_logs;

use std::path::Path;
use std::time::Instant;

use serde_json::Value;

use crate::providers::local::disk_cleanup::janitor::pass::cleaners::summary::select_outcome;
use crate::providers::local::disk_cleanup::janitor::pass::cleaners::{run_cleaners, PassInputs};
use crate::providers::local::disk_cleanup::janitor::pass::lock::file::ExclusiveLock;
use crate::providers::local::disk_cleanup::janitor::pass::once::finish::finish;
use crate::providers::local::disk_cleanup::janitor::pass::service_logs::empty_service_logs;
use crate::providers::local::disk_cleanup::janitor::policy::resolve_target;
use crate::providers::local::disk_cleanup::janitor::state::error::JanitorError;
use crate::providers::local::disk_cleanup::janitor::state::report::CleanupReport;
use crate::providers::local::disk_cleanup::release_store;
use crate::providers::local::disk_cleanup::rule::read_volume;

/// The post-lock half of a pass. Split out so tests can inject the canonical
/// registry document and a fabricated home without touching the store or the
/// real `$HOME`. `_lock` holds the exclusive run lock through candidate
/// enumeration and deletion.
///
/// Below the rule's threshold an enforcing pass deletes nothing and reports
/// `healthy_noop`. At or above it, every cleaner runs with nothing held back.
/// A preview runs every cleaner whatever the volume reads, counting what a
/// pass at the threshold would remove and removing none of it.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_with_lock(
    home: &Path,
    state_dir: &Path,
    _lock: ExclusiveLock,
    registry: Result<Value, JanitorError>,
    mut report: CleanupReport,
    started: Instant,
    attempted_at: f64,
    preview: bool,
    log_fn: &mut dyn FnMut(&str),
) -> Value {
    // A preview leaves no trace: the state file is the janitor's record of
    // real passes.
    let persist = if preview { None } else { Some(state_dir) };
    // Which release versions the fleet DECLARES, and where this host records
    // Weles runs. The registry is the only place a version another host needs
    // is written down, and the release-store cleaner runs on whichever host
    // carries the store — usually not the host that runs the binary.
    let (declared_release_versions, target) = match &registry {
        Ok(data) => (
            Some(release_store::declared_versions(data)),
            resolve_target(data, &report.hostname),
        ),
        Err(exc) => (
            None,
            Err(JanitorError::os(&format!("registry unreadable: {exc}"))),
        ),
    };
    let target = match target {
        Ok(target) => Some(target),
        Err(exc) => {
            report.add_error("registry", &exc);
            None
        }
    };
    if !preview {
        empty_service_logs(home, log_fn);
    }
    let weles_recordings_dir = target
        .as_ref()
        .and_then(|target| target.weles.as_ref())
        .and_then(|weles| weles.recordings_dir.clone());
    let inputs = PassInputs {
        enforcing: !preview,
        declared_release_versions: declared_release_versions.as_ref(),
        weles_recordings_dir: weles_recordings_dir.as_deref(),
    };
    if let Err(exc) = run_cleaners(home, &inputs, &mut report).await {
        report.add_error("runtime", &exc);
    }
    match read_volume(home) {
        Ok(after) => select_outcome(!preview, &mut report, after),
        Err(exc) => {
            report.add_error("volume", &exc);
            report.outcome = "volume_unreadable".to_string();
        }
    }
    finish(report, started, Some(home), persist, attempted_at, log_fn)
}
