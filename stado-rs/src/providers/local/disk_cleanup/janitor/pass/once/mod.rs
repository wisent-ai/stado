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
use crate::providers::local::disk_cleanup::janitor::policy::roots::free_bytes;
use crate::providers::local::disk_cleanup::janitor::state::error::JanitorError;
use crate::providers::local::disk_cleanup::janitor::state::report::build::epoch_now;
use crate::providers::local::disk_cleanup::janitor::state::report::CleanupReport;
use crate::providers::local::disk_cleanup::janitor::state::ControlUpdateAuthority;

// ---------------------------------------------------------------------------
// run_cleanup_once
// ---------------------------------------------------------------------------
/// The shared body of [`run_cleanup_once`] and [`preview_cleanup_once`].
pub(crate) async fn cleanup_once(
    active_job_count: i64,
    force: bool,
    preview: bool,
    requested_target: bool,
    writer: CleanupWriter,
    log_fn: &mut dyn FnMut(&str),
) -> Value {
    let started = Instant::now();
    let attempted_at = epoch_now();
    let hostname = crate::providers::vast::system_hostname();
    let mut report = CleanupReport::base(active_job_count, &hostname);
    report.writer = writer.as_str();
    report.writer_version = crate::binary::build_identity::BUILD_IDENTITY;

    // Python's outer `except BaseException` half: any failure before the
    // policy resolves lands in `runtime` and leaves the default outcome.
    let home = match secure_home(&crate::config_file::expand_tilde("~")) {
        Ok(home) => home,
        Err(exc) => {
            report.add_error("runtime", &exc);
            report.outcome = "invalid_or_unavailable_policy".to_string();
            return finish(
                report,
                started,
                None,
                None,
                attempted_at,
                ControlUpdateAuthority::Preserve,
                log_fn,
            );
        }
    };
    let state_dir = match ensure_state_dir(&home) {
        Ok(dir) => dir,
        Err(exc) => {
            report.add_error("runtime", &exc);
            report.outcome = "invalid_or_unavailable_policy".to_string();
            return finish(
                report,
                started,
                Some(&home),
                None,
                attempted_at,
                ControlUpdateAuthority::Preserve,
                log_fn,
            );
        }
    };
    let persist = if preview {
        None
    } else {
        Some(state_dir.as_path())
    };
    // Resolve the canonical policy before taking the exclusive janitor lock.
    // It has no filesystem side effects, and an unavailable authority fails
    // closed before blocking another cleanup process.
    //
    // The workdir keep-list is different: its candidate names must be captured
    // under the same lock that protects deletion. `run_with_lock` performs that
    // candidate-bounded authority read immediately before the workdir cleaner.
    let store_wait = Instant::now();
    let registry = fetch_canonical_registry().await;
    report.store_wait_ms = store_wait.elapsed().as_millis().min(i64::MAX as u128) as i64;
    // Only the kernel can establish ownership. Age is diagnostic information,
    // never permission to replace a live lock or run a second cleanup.
    let writer_label = writer.as_str();
    let lock = match acquire_lock_state(&state_dir, writer_label) {
        Ok(LockState::Held(lock)) => lock,
        Ok(LockState::Busy { holder }) => {
            report.lock_busy = true;
            match holder {
                _ if busy::describe_workloads(
                    &state_dir,
                    &home,
                    &registry,
                    !preview,
                    &mut report,
                    log_fn,
                ) => {}
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
            // A busy observation must not erase the state the holder is
            // continuing. The policy-bound reclaim intent decides whether the
            // next pass continues to target, and the build-cache walker relies
            // on its resume cursor. Replacing either with this observation
            // made the next writer stop at the low watermark and restart the
            // interrupted scan at the root.
            preserve_previous_report(&state_dir, &mut report);
            if let Ok(free) = free_bytes(&home) {
                report.free_bytes_before = Some(free);
                report.free_bytes_after = Some(free);
            }
            // `persist`, not `None`: a pass prevented by a live holder is the
            // fact the stall arithmetic needs most, and without it forty
            // prevented passes and forty passes that never ran leave an
            // identical, empty record. Kept from origin/main's change to this
            // same branch of the function.
            return finish(
                report,
                started,
                Some(&home),
                persist,
                attempted_at,
                ControlUpdateAuthority::Preserve,
                log_fn,
            );
        }
        Err(exc) => {
            report.add_error("runtime", &exc);
            report.outcome = "invalid_or_unavailable_policy".to_string();
            return finish(
                report,
                started,
                Some(&home),
                persist,
                attempted_at,
                ControlUpdateAuthority::Preserve,
                log_fn,
            );
        }
    };
    let predecessor_holders = match retired_locks_active(&state_dir, &lock.file) {
        Ok(holders) => holders,
        Err(error) => {
            report.add_error("lock_recovery", &error);
            vec!["a retired lock this pass could not examine".to_string()]
        }
    };
    let predecessor_active = !predecessor_holders.is_empty();
    if predecessor_active {
        let detail = format!(
            "a retired cleanup lock inode is still held by {}; this pass persists diagnostics without scanning or deleting",
            predecessor_holders.join("; ")
        );
        log_fn(&format!("disk cleanup: {detail}"));
        report.add_error("lock_predecessor_active", &JanitorError::os(&detail));
    }
    if predecessor_active {
        report.outcome = "lock_recovery_report_only".to_string();
        preserve_previous_report(&state_dir, &mut report);
        if let Ok(free) = free_bytes(&home) {
            report.free_bytes_before = Some(free);
            report.free_bytes_after = Some(free);
        }
        return finish(
            report,
            started,
            Some(&home),
            persist,
            attempted_at,
            ControlUpdateAuthority::Preserve,
            log_fn,
        );
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
        force,
        requested_target,
        preview,
        log_fn,
    )
    .await
}
