//! The shared body of one cleanup pass, from the lock inward.

mod busy;
pub(crate) mod entry;
pub(crate) mod finish;
pub(crate) mod keep_list;

use std::time::Instant;

use serde_json::Value;

use crate::providers::local::disk_cleanup::janitor::pass::lock::file::LockState;
use crate::providers::local::disk_cleanup::janitor::pass::lock::holds;
use crate::providers::local::disk_cleanup::janitor::pass::lock::ownership::{
    acquire_lock_state, retired_locks_active,
};
use crate::providers::local::disk_cleanup::janitor::pass::lock::{ensure_state_dir, secure_home};
use crate::providers::local::disk_cleanup::janitor::pass::once::entry::CleanupWriter;
use crate::providers::local::disk_cleanup::janitor::pass::once::finish::{
    finish, preserve_previous_report,
};
use crate::providers::local::disk_cleanup::janitor::pass::run_with_lock;
use crate::providers::local::disk_cleanup::janitor::policy::fetch_canonical_registry;
use crate::providers::local::disk_cleanup::janitor::state::error::JanitorError;
use crate::providers::local::disk_cleanup::janitor::state::report::build::{epoch_now, utc_now};
use crate::providers::local::disk_cleanup::janitor::state::report::CleanupReport;
use crate::providers::local::disk_cleanup::rule::read_volume;

/// The shared body of [`run_cleanup_once`] and [`preview_cleanup_once`].
pub(crate) async fn cleanup_once(
    active_job_count: i64,
    preview: bool,
    writer: CleanupWriter,
    log_fn: &mut dyn FnMut(&str),
) -> Value {
    let started = Instant::now();
    let attempted_at = epoch_now();
    let hostname = crate::providers::vast::system_hostname();
    let mut report = CleanupReport::base(active_job_count, &hostname);
    report.writer = writer.as_str();
    report.writer_version = crate::binary::build_identity::BUILD_IDENTITY;

    let home = match secure_home(&crate::config_file::expand_tilde("~")) {
        Ok(home) => home,
        Err(exc) => {
            report.add_error("runtime", &exc);
            return finish(report, started, None, None, attempted_at, log_fn);
        }
    };
    let state_dir = match ensure_state_dir(&home) {
        Ok(dir) => dir,
        Err(exc) => {
            report.add_error("runtime", &exc);
            return finish(report, started, Some(&home), None, attempted_at, log_fn);
        }
    };
    let persist = if preview {
        None
    } else {
        Some(state_dir.as_path())
    };
    // The reading the rule judges, taken before the lock so a pass that finds
    // the lock held by running workloads knows whether to ask for its turn.
    match read_volume(&home) {
        Ok(reading) => report.record_reading(reading),
        Err(exc) => {
            report.add_error("volume", &exc);
            return finish(report, started, Some(&home), persist, attempted_at, log_fn);
        }
    }
    // Below the threshold there is nothing to do: no registry read, no lock,
    // no cleaner, and no service log is touched.
    if !preview && report.pressure_active != Some(true) {
        report.outcome = "healthy_noop".to_string();
        report.last_success_at = Some(utc_now());
        return finish(report, started, Some(&home), persist, attempted_at, log_fn);
    }
    // The registry says which release versions the fleet runs and where this
    // host's Weles worker records. It is read before the exclusive lock: it
    // has no filesystem side effects. The workdir keep-list is different: its
    // candidate names must be captured under the same lock that protects
    // deletion, so the store cleaners read it themselves.
    let store_wait = Instant::now();
    let registry = fetch_canonical_registry().await;
    report.store_wait_ms = store_wait.elapsed().as_millis().min(i64::MAX as u128) as i64;
    // Only the kernel can establish ownership. Age is diagnostic information,
    // never permission to replace a live lock or run a second cleanup.
    let lock = match acquire_lock_state(&state_dir, writer.as_str()) {
        Ok(LockState::Held(lock)) => lock,
        Ok(LockState::Busy { holder }) => {
            report.lock_busy = true;
            match holder {
                _ if busy::describe_workloads(&state_dir, !preview, &mut report, log_fn) => {}
                Some(holder) => {
                    let age = epoch_now() - holder.acquired_at;
                    let detail = format!(
                        "kernel lock held; recorded pid {} ({} {}), acquired {age:.0}s ago",
                        holder.pid, holder.writer, holder.writer_version,
                    );
                    log_fn(&format!("disk cleanup: lock {detail}"));
                    report.add_error("lock_busy", &JanitorError::os(&detail));
                    report.outcome = "lock_busy".to_string();
                }
                None => {
                    log_fn(
                        "disk cleanup: kernel lock is held and no readable holder record is available",
                    );
                    report.add_error(
                        "lock_busy",
                        &JanitorError::os("kernel lock held; no readable holder record"),
                    );
                    report.outcome = "lock_busy_unattributed".to_string();
                }
            }
            preserve_previous_report(&state_dir, &mut report);
            // `persist`, not `None`: a pass prevented by a live holder is the
            // fact the stall arithmetic needs most.
            return finish(report, started, Some(&home), persist, attempted_at, log_fn);
        }
        Err(exc) => {
            report.add_error("runtime", &exc);
            return finish(report, started, Some(&home), persist, attempted_at, log_fn);
        }
    };
    let predecessor_holders = match retired_locks_active(&state_dir, &lock.file) {
        Ok(holders) => holders,
        Err(error) => {
            report.add_error("lock_recovery", &error);
            vec!["a retired lock this pass could not examine".to_string()]
        }
    };
    if !predecessor_holders.is_empty() {
        let detail = format!(
            "a retired cleanup lock inode is still held by {}; this pass persists diagnostics without scanning or deleting",
            predecessor_holders.join("; ")
        );
        log_fn(&format!("disk cleanup: {detail}"));
        report.add_error("lock_predecessor_active", &JanitorError::os(&detail));
        report.outcome = "lock_recovery_report_only".to_string();
        preserve_previous_report(&state_dir, &mut report);
        return finish(report, started, Some(&home), persist, attempted_at, log_fn);
    }

    // The exclusive hold answers any turn this janitor asked running
    // workloads for; they may take their shared holds again once it ends. A
    // preview deletes nothing, so it leaves the request standing.
    if !preview {
        holds::clear_turn(&state_dir);
    }
    run_with_lock(
        &home,
        &state_dir,
        lock,
        registry,
        report,
        started,
        attempted_at,
        preview,
        log_fn,
    )
    .await
}
