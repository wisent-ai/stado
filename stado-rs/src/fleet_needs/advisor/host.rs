//! One host's needs, read from its own publication: storage, and room for
//! placed workloads.

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{diag_number, Evidence, Need, NeedKind, Severity};
use crate::fleet_needs::unmet::{UnmetPlacement, UnmetReason};
use crate::primitives::constants;
use crate::queue::capacity::Publication;
use crate::targets::ComputeTarget;

pub(super) fn host_needs(
    target: &ComputeTarget,
    publication: Option<&Publication>,
    unmet: &[UnmetPlacement],
    now: DateTime<Utc>,
) -> Vec<Need> {
    let mut needs = Vec::new();
    let Some(publication) = publication else {
        return needs;
    };
    if publication.stale(now) {
        return needs;
    }
    let payload = &publication.payload;
    let age = publication
        .age_seconds(now)
        .map(|age| format!("{age}s ago"))
        .unwrap_or_else(|| "at an unknown time".to_string());
    let reason = payload
        .get("diag")
        .and_then(|diag| diag.get("admission_reason"))
        .and_then(Value::as_str)
        .unwrap_or("");
    needs.extend(storage_need(target, payload, &age));
    needs.extend(room_need(target, payload, &age, reason, unmet));
    needs
}

/// A volume at the disk-full threshold, as the host published it.
fn storage_need(target: &ComputeTarget, payload: &Value, age: &str) -> Option<Need> {
    let full = payload
        .get("diag")
        .and_then(|diag| diag.get("disk_pressure_active"))
        .and_then(Value::as_bool)?;
    if !full {
        return None;
    }
    let free = diag_number(payload, "free_disk_gb");
    let used = diag_number(payload, "disk_used_percent");
    let threshold = crate::providers::local::disk_cleanup::rule::DISK_FULL_PERCENT;
    let fmt =
        |value: Option<f64>| value.map_or_else(|| "unknown".to_string(), |v| format!("{v:.1}"));
    Some(Need {
        need: NeedKind::Storage,
        target: Some(target.name.clone()),
        platform: None,
        severity: Severity::High,
        summary: format!(
            "{} is at the {threshold}% disk-full threshold: {}% used, {} GiB free",
            target.name,
            fmt(used),
            fmt(free)
        ),
        evidence: vec![Evidence::new(
            "capacity",
            format!(
                "{} published {age}: disk_pressure_active true, disk_used_percent {}, free_disk_gb {}",
                target.name,
                fmt(used),
                fmt(free)
            ),
        )],
        suggestion: format!(
            "the janitor on {0} deletes everything the fleet put there; what remains is the user's data — add storage to {0}, or read `stado space report {0}` for what holds it",
            target.name
        ),
    })
}

/// Reservations exhausted the host now, or refusals said it was full often.
fn room_need(
    target: &ComputeTarget,
    payload: &Value,
    age: &str,
    reason: &str,
    unmet: &[UnmetPlacement],
) -> Option<Need> {
    let refused_full = unmet
        .iter()
        .filter(|record| {
            matches!(
                record.reason,
                UnmetReason::ReservationsExhausted | UnmetReason::CapacityExhausted
            ) && record.candidates.iter().any(|c| c.target == target.name)
        })
        .count();
    if reason != "reservations_exhausted" && refused_full < constants::NEEDS_REFUSALS_FOR_CPU {
        return None;
    }
    let running_workloads = payload
        .get("running_workloads")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let cores = payload
        .get("available_cpu_cores")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    Some(Need {
        need: NeedKind::Cpu,
        target: Some(target.name.clone()),
        platform: None,
        severity: Severity::Medium,
        summary: format!(
            "{} runs out of room for placed workloads: {running_workloads} held now, {refused_full} placement(s) refused in the window",
            target.name
        ),
        evidence: vec![
            Evidence::new(
                "capacity",
                format!(
                    "{} published {age}: available_cpu_cores {cores}, running_workloads {running_workloads}, admission_reason {}",
                    target.name,
                    if reason.is_empty() { "none" } else { reason }
                ),
            ),
            Evidence::new(
                "unmet",
                format!(
                    "{refused_full} placement(s) refused on {} as full in the window",
                    target.name
                ),
            ),
        ],
        suggestion: format!("add cores or a second machine beside {}", target.name),
    })
}
