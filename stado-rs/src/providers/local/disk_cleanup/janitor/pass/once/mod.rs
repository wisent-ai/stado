//! The shared body of one cleanup pass, from the lock inward.

pub(crate) mod entry;
pub(crate) mod finish;
pub(crate) mod keep_list;

use std::time::{Duration, Instant};

use serde_json::Value;

use crate::primitives::constants;
use crate::providers::local::disk_cleanup::janitor::pass::lock::file::LockState;
use crate::providers::local::disk_cleanup::janitor::pass::lock::takeover::{
    acquire_lock_state, pid_alive, retired_locks_active,
};
use crate::providers::local::disk_cleanup::janitor::pass::lock::{ensure_state_dir, secure_home};
use crate::providers::local::disk_cleanup::janitor::pass::once::entry::CleanupWriter;
use crate::providers::local::disk_cleanup::janitor::pass::once::finish::{
    finish, preserve_previous_report,
};
use crate::providers::local::disk_cleanup::janitor::pass::run_with_lock;
use crate::providers::local::disk_cleanup::janitor::policy::roots::free_bytes;
use crate::providers::local::disk_cleanup::janitor::policy::{
    fetch_canonical_registry, resolve_canonical_policy,
};
use crate::providers::local::disk_cleanup::janitor::state::error::JanitorError;
use crate::providers::local::disk_cleanup::janitor::state::report::build::epoch_now;
use crate::providers::local::disk_cleanup::janitor::state::report::CleanupReport;
use crate::providers::local::disk_cleanup::janitor::state::ControlUpdateAuthority;
use crate::providers::local::disk_cleanup::janitor::{DEADLINE_SECONDS, LOCK_TAKEOVER_GRACE_S};

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
    // candidate-bounded authority read immediately before the workdir cleaner,
    // inside both the store-read budget and the pass deadline.
    let store_wait = Instant::now();
    let input_budget = Duration::from_secs(constants::AGENT_STORE_READ_TIMEOUT_S);
    let registry = match tokio::time::timeout(input_budget, fetch_canonical_registry()).await {
        Ok(result) => result,
        Err(_) => Err(JanitorError::timeout(&format!(
            "canonical registry did not answer within {}s",
            constants::AGENT_STORE_READ_TIMEOUT_S
        ))),
    };
    report.store_wait_ms = store_wait.elapsed().as_millis().min(i64::MAX as u128) as i64;
    // The lock is taken with a stated deadline, and a hold past its own
    // deadline is answered rather than waited out. `lock_busy` used to be the
    // only answer this function had for "somebody else has it", and that is
    // how a host spent nine and a half hours with cleanup disabled while its
    // gate said `disk_cleanup_stalled` and nothing said WHO or FOR HOW LONG.
    let pass_seconds = DEADLINE_SECONDS.max(
        registry
            .as_ref()
            .ok()
            .and_then(|data| resolve_canonical_policy(data, &report.hostname).ok())
            .and_then(|(_, policy, _, _)| policy.max_pass_seconds)
            .filter(|seconds| *seconds > 0)
            .map_or(DEADLINE_SECONDS, |seconds| seconds as f64),
    );
    let mut taken_over = false;
    let writer_label = writer.as_str().to_string();
    let lock = match acquire_lock_state(&state_dir, pass_seconds, &writer_label) {
        Ok(LockState::Held(lock)) => lock,
        Ok(LockState::TakenOver {
            lock,
            from_pid,
            overdue_seconds,
        }) => {
            taken_over = true;
            let detail = if from_pid == 0 {
                format!(
                    "took the janitor run lock from a predecessor that left no holder record, \
                     {:.0}s past the declared pass deadline plus the \
                     {LOCK_TAKEOVER_GRACE_S:.0}s grace as the lock file's own age reports it; \
                     this pass runs in report mode, and enforcement stays disabled until the \
                     retired predecessor inode no longer has a kernel lock",
                    overdue_seconds
                )
            } else {
                let liveness = if pid_alive(from_pid) {
                    "still running and not progressing"
                } else {
                    "gone"
                };
                format!(
                    "took the janitor run lock from pid {from_pid} ({liveness}), {:.0}s past the \
                     deadline that holder recorded plus the {LOCK_TAKEOVER_GRACE_S:.0}s grace; \
                     this pass runs in report mode, and enforcement stays disabled until the \
                     retired predecessor inode no longer has a kernel lock",
                    overdue_seconds
                )
            };
            log_fn(&format!("disk cleanup: {detail}"));
            report.add_error("lock_taken_over", &JanitorError::os(&detail));
            lock
        }
        Ok(LockState::Busy { holder }) => {
            report.lock_busy = true;
            match holder {
                Some(holder) => {
                    let age = epoch_now() - holder.acquired_at;
                    let remaining = holder.deadline_at - epoch_now();
                    // Recognizable, not silent: an operator reading a report
                    // now learns which process holds the lock, how long it has
                    // held it and whether it is inside its own budget.
                    let detail = format!(
                        "held for {age:.0}s by pid {} ({} {}), {:.0}s of its declared budget left",
                        holder.pid,
                        holder.writer,
                        holder.writer_version,
                        remaining.max(0.0)
                    );
                    log_fn(&format!("disk cleanup: lock {detail}"));
                    report.add_error("lock_busy", &JanitorError::os(&detail));
                    report.outcome = "lock_busy".to_string();
                }
                None => {
                    // No record at all: a holder from a build older than this
                    // one, or a lock file created by hand. Say that too.
                    log_fn(
                        "disk cleanup: lock is held by a process that left no holder record; its \
                         deadline is unknown, so it will not be taken over",
                    );
                    report.add_error(
                        "lock_busy",
                        &JanitorError::os("held with no holder record; deadline unknown"),
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
    let predecessor_active = match retired_locks_active(&state_dir, &lock.file) {
        Ok(active) => active,
        Err(error) => {
            report.add_error("lock_recovery", &error);
            true
        }
    };
    if predecessor_active {
        let detail =
            "a retired cleanup lock inode is still held; this pass persists diagnostics without scanning or deleting";
        log_fn(&format!("disk cleanup: {detail}"));
        report.add_error("lock_predecessor_active", &JanitorError::os(detail));
    }
    if taken_over || predecessor_active {
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
