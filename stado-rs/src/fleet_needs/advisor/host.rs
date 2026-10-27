//! One host's needs, read from its own publication: storage, and room for
//! placed workloads.

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{diag_number, Evidence, Need, NeedKind, Severity};
use crate::fleet_needs::unmet::{UnmetPlacement, UnmetReason};
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
    let age = match publication.age_seconds(now) {
        Some(age) => format!("{age}s ago"),
        None => "with no publication time".to_string(),
    };
    let reason = payload
        .get("diag")
        .and_then(|diag| diag.get("admission_reason"))
        .and_then(Value::as_str);
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
    let threshold = crate::providers::local::space_reclaim::rule::DISK_FULL_PERCENT;
    // A figure the agent could not measure is published null with its cause
    // beside it as `<name>_unread`; the cause is the evidence, not a blank.
    let fmt = |value: Option<f64>, name: &str| match value {
        Some(value) => format!("{value:.1}"),
        None => match payload
            .get("diag")
            .and_then(|diag| diag.get(format!("{name}_unread")))
            .and_then(Value::as_str)
        {
            Some(cause) => format!("unmeasured ({cause})"),
            None => "not published".to_string(),
        },
    };
    // The volume reading's cause is published as `disk_unread`.
    let used = fmt(used, "disk");
    let free = fmt(free, "free_disk_gb");
    Some(Need {
        need: NeedKind::Storage,
        target: Some(target.name.clone()),
        platform: None,
        severity: Severity::High,
        summary: format!(
            "{} is at the {threshold}% disk-full threshold: {}% used, {} GiB free",
            target.name,
            used,
            free
        ),
        evidence: vec![Evidence::new(
            "capacity",
            format!(
                "{} published {age}: disk_pressure_active true, disk_used_percent {}, free_disk_gb {}",
                target.name,
                used,
                free
            ),
        )],
        suggestion: format!(
            "the janitor on {0} deletes everything the fleet put there; what remains is the user's data — add storage to {0}, or read `stado space report {0}` for what holds it",
            target.name
        ),
    })
}

/// Reservations exhausted the host now, or a placement was refused as full.
fn room_need(
    target: &ComputeTarget,
    payload: &Value,
    age: &str,
    reason: Option<&str>,
    unmet: &[UnmetPlacement],
) -> Option<Need> {
    let refused_full: Vec<&UnmetPlacement> = unmet
        .iter()
        .filter(|record| {
            matches!(
                record.reason,
                UnmetReason::ReservationsExhausted | UnmetReason::CapacityExhausted
            ) && record.candidates.iter().any(|c| c.target == target.name)
        })
        .collect();
    if reason != Some("reservations_exhausted") && refused_full.is_empty() {
        return None;
    }
    let refused_full = refused_full.len();
    // A count the host's capacity row does not publish is said so, never zero.
    let published = |field: &str| match payload.get(field).and_then(Value::as_i64) {
        Some(count) => count.to_string(),
        None => "not published".to_string(),
    };
    let running_workloads = published("running_workloads");
    let cores = published("available_cpu_cores");
    let reason = match reason {
        Some(reason) => reason,
        None => "none recorded",
    };
    Some(Need {
        need: NeedKind::Cpu,
        target: Some(target.name.clone()),
        platform: None,
        severity: Severity::Medium,
        summary: format!(
            "{} runs out of room for placed workloads: {running_workloads} held now, {refused_full} placement(s) refused as full",
            target.name
        ),
        evidence: vec![
            Evidence::new(
                "capacity",
                format!(
                    "{} published {age}: available_cpu_cores {cores}, running_workloads {running_workloads}, admission_reason {reason}",
                    target.name,
                ),
            ),
            Evidence::new(
                "unmet",
                format!(
                    "{refused_full} recorded placement(s) refused on {} as full",
                    target.name
                ),
            ),
        ],
        suggestion: format!("add cores or a second machine beside {}", target.name),
    })
}
