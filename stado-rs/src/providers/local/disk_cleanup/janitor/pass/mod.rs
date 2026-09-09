//! One bounded cleanup pass: the exclusive run lock, the cleaners it
//! authorizes, and the outcome it reports.

pub(crate) mod cleaners;
pub(crate) mod lock;
pub(crate) mod once;
pub(crate) mod service_logs;

use std::path::Path;
use std::time::Instant;

use serde_json::Value;

use crate::providers::local::disk_cleanup::janitor::pass::cleaners::run_cleaners;
use crate::providers::local::disk_cleanup::janitor::pass::cleaners::summary::{
    select_outcome, summarize_scan,
};
use crate::providers::local::disk_cleanup::janitor::pass::lock::file::ExclusiveLock;
use crate::providers::local::disk_cleanup::janitor::pass::once::finish::finish;
use crate::providers::local::disk_cleanup::janitor::pass::service_logs::rotate_service_logs;
use crate::providers::local::disk_cleanup::janitor::policy::resolve_canonical_policy;
use crate::providers::local::disk_cleanup::janitor::policy::roots::free_bytes;
use crate::providers::local::disk_cleanup::janitor::state::error::JanitorError;
use crate::providers::local::disk_cleanup::janitor::state::report::build::utc_now;
use crate::providers::local::disk_cleanup::janitor::state::report::CleanupReport;
use crate::providers::local::disk_cleanup::janitor::state::{
    read_state, reclaim_intent_digest, reclaim_intent_outcome, writer_last_attempt,
    ControlUpdateAuthority,
};
use crate::providers::local::disk_cleanup::janitor::GIB;
use crate::providers::local::disk_cleanup::{build_caches, release_store};

/// The post-lock half of `run_cleanup_once` (policy resolution through
/// outcome selection). Split out so tests can inject the canonical registry
/// document and a fabricated home without touching GCS or the real `$HOME`.
/// `_lock` holds the exclusive run lock through candidate enumeration,
/// authoritative state reads, and deletion.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_with_lock(
    home: &Path,
    state_dir: &Path,
    _lock: ExclusiveLock,
    registry: Result<Value, JanitorError>,
    mut report: CleanupReport,
    started: Instant,
    attempted_at: f64,
    force: bool,
    requested_target: bool,
    // Plan only: pin an `enforce` policy down to the janitor's own `report`
    // mode and persist nothing. See `preview_cleanup_once`.
    preview: bool,
    log_fn: &mut dyn FnMut(&str),
) -> Value {
    // A preview leaves no trace. The state file is the janitor's record of
    // REAL passes: writing it would advance this writer's attempt stamp, so an
    // operator asking what a cleanup WOULD delete would have silently
    // delayed the cleanup that does.
    let persist = if preview { None } else { Some(state_dir) };
    // Which release versions the fleet DECLARES, taken from the same document
    // this pass resolves its policy from, before that document is consumed.
    // The registry is the only place a version another host needs is written
    // down, and the release-store cleaner runs on whichever host carries the
    // store — usually not the host that runs the binary.
    let declared_release_versions = registry
        .as_ref()
        .ok()
        .map(release_store::declared_versions)
        .unwrap_or_default();
    let (target, mut policy, digest, policy_defaulted) =
        match registry.and_then(|data| resolve_canonical_policy(&data, &report.hostname)) {
            Ok(value) => value,
            Err(exc) => {
                report.add_error("policy", &exc);
                return finish(
                    report,
                    started,
                    Some(home),
                    persist,
                    attempted_at,
                    ControlUpdateAuthority::Owner,
                    log_fn,
                );
            }
        };
    // `enforce` is the only mode that deletes. The janitor's own `report`
    // mode walks the identical scan and counts every eligible item without
    // unlinking one — `hf::run_hf` and `weles::scan_weles` both return
    // before their removal step whenever the mode is not `"enforce"` — so
    // preview and lock recovery are this pass with that one word changed,
    // not second implementations of the policy.
    //
    // `off` and `report` policies are left exactly as the registry states.
    if preview && policy.mode == "enforce" {
        policy.mode = "report".to_string();
    }
    report.target_name = Some(target.name);
    report.policy_digest = Some(digest.clone());
    report.mode = Some(policy.mode.clone());
    report.check_interval_seconds = Some(policy.check_interval_seconds);
    report.low_bytes = Some(policy.low_free_gb * GIB);
    report.target_bytes = Some(policy.target_free_gb * GIB);
    report.policy_defaulted = policy_defaulted;

    let previous = match read_state(state_dir) {
        Ok(value) => value,
        Err(exc) => {
            report.add_error("state_read", &exc);
            return finish(
                report,
                started,
                Some(home),
                persist,
                attempted_at,
                ControlUpdateAuthority::Owner,
                log_fn,
            );
        }
    };
    let previous_report = previous.get("report").filter(|r| r.is_object()).cloned();
    report.last_success_at = previous_report
        .as_ref()
        .and_then(|r| r.get("last_success_at"))
        .and_then(|v| v.as_str().map(str::to_string));
    let reclaim_digest = reclaim_intent_digest(&previous);
    let same_reclaim_policy = reclaim_digest == Some(digest.as_str());
    // A legacy position alone cannot resume without replaying prior levels.
    // The next actual scan migrates it to a durable unvisited frontier, but a
    // checkpoint belonging to another policy must never be attached here.
    report.builds_resume_from = same_reclaim_policy
        .then(|| {
            previous_report
                .as_ref()
                .and_then(|r| r.get("build_caches_resume_from"))
                .and_then(|v| v.as_str().map(str::to_string))
        })
        .flatten();
    report.builds_cursor = same_reclaim_policy
        .then(|| build_caches::BuildCachesCursor::from_state(&previous))
        .flatten();
    report.backup_cursor = same_reclaim_policy
        .then(|| {
            crate::providers::local::disk_cleanup::backup_twins::cursor::BackupCursor::from_state(
                &previous,
            )
        })
        .flatten();
    let before = match free_bytes(home) {
        Ok(free) => free,
        Err(exc) => {
            report.add_error("runtime", &exc);
            report.outcome = "invalid_or_unavailable_policy".to_string();
            return finish(
                report,
                started,
                Some(home),
                Some(state_dir),
                attempted_at,
                ControlUpdateAuthority::Owner,
                log_fn,
            );
        }
    };
    report.free_bytes_before = Some(before);
    report.free_bytes_after = Some(before);
    let requested_reclaim = requested_target && before < policy.target_free_gb * GIB;
    let continuing_reclaim =
        requested_reclaim || (same_reclaim_policy && before < policy.target_free_gb * GIB);
    let immediate_reclaim = continuing_reclaim
        && (requested_reclaim || reclaim_intent_outcome(&previous) == Some("cap_reached"));
    let below_low = before < policy.low_free_gb * GIB;
    report.pressure_active = Some(below_low || continuing_reclaim);
    // THIS writer's last attempt, not the file's.
    //
    // This read used to be `previous["last_attempt_at"]` - the last attempt by
    // anyone - so any writer's stamp gated every writer. On 2026-08-31
    // charless-mac-mini had two janitors: the queue agent's in-process pass and
    // a standalone `disk-cleanup` unit on its own timer. With the thresholds
    // raised to 40/42 GiB against 31.2 GiB free, the agent reported
    // `disk_pressure_active: true`, `errors: []`, policy resolved, and all six
    // cleaners `scanned 0` - because the other process had stamped the file
    // within the interval. Pressure active, policy resolved, nothing scanned.
    //
    // The gate returns before the first scanner AND before `run_with_lock`
    // reaches the lock, so the lock cannot mediate it: the lock makes two
    // janitors take turns deleting, while this made the working one never try.
    // Both are real and only this one silences a pass.
    //
    // Removing a redundant unit does not fix this. `stado disk-cleanup --once`
    // is a supported operator command that writes the same file, so one manual
    // run would otherwise silence the agent's janitor for a full interval on
    // any host.
    // The interval normally paces observations above the low watermark.
    // Capped work is a bounded frontier and continues immediately; a
    // blocked/error pass retains its intent but waits for the writer's normal
    // interval so an unchanged external blocker cannot create a tight loop.
    // Below low, cleanup remains immediate. Concurrency is mediated by the
    // lock below rather than by this stamp.
    let last_attempt = writer_last_attempt(&previous, report.writer);
    if !force
        && !below_low
        && !immediate_reclaim
        && last_attempt.is_some()
        && attempted_at - last_attempt.unwrap_or_default() < policy.check_interval_seconds as f64
    {
        report.outcome = "interval_noop".to_string();
        return finish(
            report,
            started,
            Some(home),
            persist,
            last_attempt.unwrap_or(attempted_at),
            ControlUpdateAuthority::Owner,
            log_fn,
        );
    }
    if !preview && policy.mode == "enforce" {
        rotate_service_logs(home, log_fn);
    }
    if policy.mode == "off" || report.pressure_active != Some(true) {
        report.outcome = "healthy_noop".to_string();
        report.last_success_at = Some(utc_now());
        return finish(
            report,
            started,
            Some(home),
            persist,
            attempted_at,
            ControlUpdateAuthority::Owner,
            log_fn,
        );
    }
    if let Err(exc) = run_cleaners(
        home,
        &policy,
        &declared_release_versions,
        attempted_at,
        &mut report,
    )
    .await
    {
        report.add_error("runtime", &exc);
        report.outcome = "invalid_or_unavailable_policy".to_string();
        return finish(
            report,
            started,
            Some(home),
            persist,
            attempted_at,
            ControlUpdateAuthority::Owner,
            log_fn,
        );
    }
    summarize_scan(&policy, &mut report);
    let after = match free_bytes(home) {
        Ok(free) => free,
        Err(exc) => {
            report.add_error("runtime", &exc);
            report.outcome = "invalid_or_unavailable_policy".to_string();
            return finish(
                report,
                started,
                Some(home),
                persist,
                attempted_at,
                ControlUpdateAuthority::Owner,
                log_fn,
            );
        }
    };
    select_outcome(&policy, &mut report, after);
    finish(
        report,
        started,
        Some(home),
        persist,
        attempted_at,
        ControlUpdateAuthority::Owner,
        log_fn,
    )
}
