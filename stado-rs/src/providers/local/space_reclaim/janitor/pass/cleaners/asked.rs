//! What the store cleaners ask the queue, asked before the exclusive lock.
//!
//! The queue answers through the object API, whose reads can stand on the
//! registry authority for a long time. Asked under the exclusive cleanup
//! lock, those reads held every build on the host for as long as they stood.
//! They are asked first, for the candidates on disk then; under the lock the
//! cleaners act on those answers only. A job work tree that appeared after
//! the asking is not in them, so it is kept as live; a job output is removed
//! only when the queue named it terminal, and a terminal job stays terminal.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Instant;

use crate::providers::local::space_reclaim::janitor::pass::once::keep_list::{
    fetch_live_job_ids, fetch_terminal_job_ids,
};
use crate::providers::local::space_reclaim::janitor::state::report::CleanupReport;
use crate::providers::local::space_reclaim::{job_outputs, queue_workdirs};

/// The queue's answers for the candidates on disk when it was asked. `None`
/// where a candidate list or the queue could not be read; that cleaner then
/// removes nothing this pass, and the cause is in the report.
pub(crate) struct Asked {
    /// The job work trees on disk when asked, and which of them were live.
    pub workdirs: Option<(BTreeSet<String>, Vec<String>)>,
    /// The job outputs the queue named terminal.
    pub terminal_outputs: Option<BTreeSet<String>>,
}

impl Asked {
    /// Every work tree to keep, for the candidates on disk under the lock:
    /// the live ones the queue named, and each one that was not on disk when
    /// the queue was asked.
    pub fn live_workdirs(&self, now: &BTreeSet<String>) -> Option<Vec<String>> {
        let (asked_for, live) = self.workdirs.as_ref()?;
        let mut keep = live.clone();
        keep.extend(now.difference(asked_for).cloned());
        Some(keep)
    }
}

/// Add the time spent waiting on the queue store to the report.
fn charge_wait(report: &mut CleanupReport, wait: Instant) {
    report.store_wait_ms = report
        .store_wait_ms
        .saturating_add(wait.elapsed().as_millis().min(i64::MAX as u128) as i64);
}

/// Ask the queue about the candidates on disk now, before any lock is taken.
pub(crate) async fn ask(home: &Path, report: &mut CleanupReport) -> Asked {
    let workdirs = match queue_workdirs::candidate_job_ids(home) {
        Ok(candidates) => {
            let wait = Instant::now();
            let live = match fetch_live_job_ids(&candidates).await {
                Ok(ids) => Some(ids),
                Err(error) => {
                    report.add_error(queue_workdirs::CLEANER, &error);
                    None
                }
            };
            charge_wait(report, wait);
            live.map(|live| (candidates, live))
        }
        Err(error) => {
            report.add_error(queue_workdirs::CLEANER, &error);
            None
        }
    };
    let status_roots = job_outputs::status_roots(home);
    let terminal_outputs = match job_outputs::candidate_job_ids(&status_roots) {
        Ok(candidates) if candidates.is_empty() => Some(BTreeSet::new()),
        Ok(candidates) => {
            let wait = Instant::now();
            let ids = match fetch_terminal_job_ids(&candidates).await {
                Ok(ids) => Some(ids),
                Err(error) => {
                    report.add_error(job_outputs::CLEANER, &error);
                    None
                }
            };
            charge_wait(report, wait);
            ids
        }
        Err(error) => {
            report.add_error(job_outputs::CLEANER, &error);
            None
        }
    };
    Asked {
        workdirs,
        terminal_outputs,
    }
}
