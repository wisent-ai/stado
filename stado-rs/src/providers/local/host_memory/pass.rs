//! One bounded memory-reclaim pass.
//!
//! The same shape as the disk janitor's pass and in the same order: resolve
//! the declaration, gate on this writer's own interval, take an exclusive
//! lock, read the host, decide against the declared watermarks, perform only
//! the repairs the registry names, read the host again, and persist one
//! report under the outcome vocabulary both passes share.
//!
//! Two writers run it — the janitor unit on its own timer and the queue
//! agent's janitor task on every tick — which is why the interval is per
//! writer and the lock is exclusive: the second arrival records `lock_busy`
//! rather than repairing a host somebody else is already repairing.

use std::collections::BTreeMap;
use std::time::Instant;

use serde_json::Value;

use super::constants;
use super::reading::{self, MemoryReading};
use super::report::{self, MemoryCaps, MemoryReport, RepairReport};
use super::schema::{
    MemoryReclaimPolicy, REPAIR_GRAPHICAL_SESSION, REPAIR_NAMES, REPAIR_REAP_RECOVERY,
    REPAIR_RESTART_UNIT,
};
use super::{policy, repairs, session, state};

/// Which process made a pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryWriter {
    /// The queue agent's janitor task.
    AgentTick,
    /// `stado disk-cleanup`, whether `--once` or under its watch unit.
    Cli,
}

impl MemoryWriter {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AgentTick => "agent-tick",
            Self::Cli => "memory-reclaim-cli",
        }
    }
}

fn utc_now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn base_report(writer: MemoryWriter, hostname: String, started_at: String) -> MemoryReport {
    MemoryReport {
        hostname,
        target_name: None,
        policy_digest: None,
        writer: writer.as_str(),
        writer_version: env!("CARGO_PKG_VERSION"),
        policy_defaulted: false,
        mode: None,
        check_interval_seconds: None,
        started_at,
        duration_ms: i64::default(),
        outcome: report::NEVER_RUN.to_string(),
        before: MemoryReading::default(),
        after: None,
        low_bytes: None,
        target_bytes: None,
        high_swap_used_pct: None,
        pressure_active: None,
        refuse_placement: false,
        placement_refusal: None,
        repairs: BTreeMap::new(),
        examined_repairs: false,
        caps: MemoryCaps::default(),
        lock_busy: false,
        active_job_count: i64::default(),
        last_success_at: None,
        errors: Vec::new(),
    }
}

fn describe(policy: &MemoryReclaimPolicy, defaulted: bool, report: &mut MemoryReport) {
    report.policy_defaulted = defaulted;
    report.mode = Some(policy.mode.clone());
    report.check_interval_seconds = Some(policy.check_interval_seconds);
    report.low_bytes = Some(policy.low_free_bytes());
    report.target_bytes = Some(policy.target_free_bytes());
    report.high_swap_used_pct = Some(policy.high_swap_used_pct);
    report.refuse_placement = policy.refuse_placement;
}

/// Resolve the declaration and execute at most one bounded pass.
///
/// Never fails: every failure mode lands in the returned report, because the
/// two writers that call this are a CLI loop and an agent task and neither
/// has anywhere to put an error that the report cannot carry.
pub async fn run_memory_pass_once(
    active_job_count: i64,
    writer: MemoryWriter,
    log_fn: &mut dyn FnMut(&str),
) -> Value {
    let started = Instant::now();
    let attempted_at = state::now_epoch_seconds();
    let hostname = crate::providers::vast::system_hostname();
    let mut report = base_report(writer, hostname.clone(), utc_now());
    report.active_job_count = active_job_count;

    let state_dir = match state::state_dir() {
        Ok(dir) => dir,
        Err(error) => {
            report.outcome = report::INVALID_OR_UNAVAILABLE_POLICY.to_string();
            report.errors.push(format!("state:{}", error.code));
            log_fn(&format!("memory: state directory unusable: {}", error.code));
            report.duration_ms = started.elapsed().as_millis() as i64;
            return report.to_value();
        }
    };

    let resolved = match policy::resolve_for_this_host(&hostname).await {
        Ok(resolved) => resolved,
        Err(error) => {
            report.outcome = report::INVALID_OR_UNAVAILABLE_POLICY.to_string();
            report.errors.push(format!("policy:{}", error.code));
            log_fn(&format!("memory: policy unresolved: {}", error.code));
            report.duration_ms = started.elapsed().as_millis() as i64;
            let value = report.to_value();
            let _ = state::write_state(&state_dir, &value, writer.as_str(), attempted_at);
            return value;
        }
    };
    report.target_name = Some(resolved.target.name.clone());
    report.policy_digest = Some(resolved.digest.clone());
    describe(&resolved.policy, resolved.defaulted, &mut report);
    let policy = resolved.policy;

    let persisted = state::read_state(&state_dir);
    report.last_success_at = persisted
        .get("report")
        .and_then(|previous| previous.get("last_success_at"))
        .and_then(Value::as_str)
        .map(str::to_string);
    if let Some(last) = state::writer_last_attempt(&persisted, writer.as_str()) {
        if attempted_at - last < policy.check_interval_seconds as f64 {
            report.outcome = report::INTERVAL_NOOP.to_string();
            report.duration_ms = started.elapsed().as_millis() as i64;
            return report.to_value();
        }
    }

    let Some(_lock) = (match state::acquire_pass_lock(&state_dir) {
        Ok(lock) => lock,
        Err(error) => {
            report.outcome = report::PARTIAL_ERROR.to_string();
            report.errors.push(format!("lock:{}", error.code));
            report.duration_ms = started.elapsed().as_millis() as i64;
            let value = report.to_value();
            let _ = state::write_state(&state_dir, &value, writer.as_str(), attempted_at);
            return value;
        }
    }) else {
        report.lock_busy = true;
        report.outcome = report::LOCK_BUSY.to_string();
        report.duration_ms = started.elapsed().as_millis() as i64;
        let value = report.to_value();
        let _ = state::write_state(&state_dir, &value, writer.as_str(), attempted_at);
        return value;
    };

    report.before = reading::read_host_memory();
    report.pressure_active = report.before.over_watermark(&policy);
    report.placement_refusal =
        report::refusal_reason(policy.refuse_placement, report.pressure_active);
    let value = finish_pass(&mut report, &policy, active_job_count, started, log_fn);
    let _ = state::write_state(&state_dir, &value, writer.as_str(), attempted_at);
    value
}

fn finish_pass(
    report: &mut MemoryReport,
    policy: &MemoryReclaimPolicy,
    active_job_count: i64,
    started: Instant,
    log_fn: &mut dyn FnMut(&str),
) -> Value {
    let over = report.pressure_active;
    if over.is_none() {
        report.outcome = report::PARTIAL_ERROR.to_string();
        report
            .errors
            .push("reading:this host answered no memory figure".to_string());
        report.duration_ms = started.elapsed().as_millis() as i64;
        return report.to_value();
    }
    if policy.mode == "off" || over != Some(true) {
        report.outcome = report::HEALTHY_NOOP.to_string();
        report.last_success_at = Some(utc_now());
        report.duration_ms = started.elapsed().as_millis() as i64;
        return report.to_value();
    }
    // Every repair here ends or restarts a process. A host that is running
    // queue work has processes that belong to that work, and a pass that
    // ended one would turn a memory report into a lost job. The declared
    // repairs wait for the tick after the host is idle, and the refusal is
    // recorded so an operator can see the pass chose not to act.
    if active_job_count > 0 && policy.mode == "enforce" {
        report.outcome = report::BLOCKED_RUNNING_JOBS.to_string();
        report.duration_ms = started.elapsed().as_millis() as i64;
        return report.to_value();
    }
    let enforce = policy.repairs_armed();
    let deadline = Instant::now() + std::time::Duration::from_secs(policy.pass_seconds());
    let mut budget = policy.max_repairs_per_pass;
    report.examined_repairs = true;
    for name in REPAIR_NAMES {
        let Some(declared) = policy.repair(name) else {
            continue;
        };
        if Instant::now() >= deadline {
            report.caps.deadline = true;
            break;
        }
        let (result, errors) = match name {
            REPAIR_RESTART_UNIT => repairs::restart_units(declared, enforce, &mut budget, log_fn),
            REPAIR_REAP_RECOVERY => repairs::run_recovery(declared, enforce, &mut budget, log_fn),
            REPAIR_GRAPHICAL_SESSION => {
                session::terminate_declared(declared, enforce, &mut budget, log_fn)
            }
            _ => (RepairReport::default(), Vec::new()),
        };
        report.errors.extend(errors);
        report.repairs.insert(name.to_string(), result);
    }
    if enforce && budget <= i64::default() {
        report.caps.repairs = true;
    }
    report.after = Some(reading::read_host_memory());
    conclude(report, policy, enforce, started)
}

fn conclude(
    report: &mut MemoryReport,
    policy: &MemoryReclaimPolicy,
    enforce: bool,
    started: Instant,
) -> Value {
    let repaired: i64 = report.repairs.values().map(|entry| entry.repaired).sum();
    let eligible: i64 = report.repairs.values().map(|entry| entry.eligible).sum();
    let at_target = report
        .after
        .as_ref()
        .and_then(|after| after.at_target(policy))
        .unwrap_or(false);
    let none = i64::default();
    report.outcome = if !report.errors.is_empty() && repaired == none {
        report::PARTIAL_ERROR.to_string()
    } else if !enforce {
        report::REPORT_ONLY.to_string()
    } else if repaired > none && at_target {
        report::RECLAIMED_TARGET.to_string()
    } else if repaired > none {
        report::RECLAIMED_PROGRESS.to_string()
    } else if report.caps.any() {
        report::CAP_REACHED.to_string()
    } else if eligible == none {
        report::NO_ELIGIBLE_ITEMS.to_string()
    } else {
        report::PARTIAL_ERROR.to_string()
    };
    if repaired > none || report.outcome == report::REPORT_ONLY {
        report.last_success_at = Some(utc_now());
    }
    report.duration_ms = started.elapsed().as_millis() as i64;
    report.to_value()
}

/// The pass budget in force, exported so the CLI can state it.
pub fn declared_pass_seconds(policy: &MemoryReclaimPolicy) -> u64 {
    policy
        .max_pass_seconds
        .and_then(|declared| u64::try_from(declared).ok())
        .unwrap_or(constants::PASS_DEADLINE_SECONDS)
}
