//! One host's needs, read from its own publication against its own
//! declarations: memory, storage, and room for placed workloads.

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{diag_number, fmt, Evidence, Need, NeedKind, Severity};
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
    needs.extend(memory_need(target, payload, &age, reason, unmet));
    needs.extend(storage_need(target, payload, &age));
    needs.extend(room_need(target, payload, &age, reason, unmet));
    needs
}

/// Pressure the host's own memory policy reports, or swap over its watermark.
fn memory_need(
    target: &ComputeTarget,
    payload: &Value,
    age: &str,
    reason: &str,
    unmet: &[UnmetPlacement],
) -> Option<Need> {
    let available = diag_number(payload, "memory_available_gb");
    let total = diag_number(payload, "memory_total_gb");
    let low = diag_number(payload, "memory_low_watermark_gb");
    let swap = diag_number(payload, "memory_swap_used_pct");
    let swap_high = diag_number(payload, "memory_swap_high_watermark_pct");
    let pressure = payload
        .get("diag")
        .and_then(|diag| diag.get("memory_pressure_active"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let swap_over = matches!((swap, swap_high), (Some(used), Some(high)) if used >= high);
    if !(pressure || swap_over) {
        return None;
    }
    let mut evidence = vec![Evidence::new(
        "capacity",
        format!(
            "{} published {age}: {} GiB available of {} GiB, swap {}% against a {}% watermark, low watermark {} GiB{}",
            target.name,
            fmt(available),
            fmt(total),
            fmt(swap),
            fmt(swap_high),
            fmt(low),
            if reason.is_empty() {
                String::new()
            } else {
                format!(", admission_reason {reason}")
            }
        ),
    )];
    let refused = unmet
        .iter()
        .filter(|record| {
            record.reason == UnmetReason::MemoryPressure
                && record.candidates.iter().any(|c| c.target == target.name)
        })
        .count();
    if refused > 0 {
        evidence.push(Evidence::new(
            "unmet",
            format!(
                "{refused} placement(s) were refused on {} for memory pressure in the window",
                target.name
            ),
        ));
    }
    let severity = if reason == "memory_pressure_active" || swap_over {
        Severity::High
    } else {
        Severity::Medium
    };
    let suggested = total.map(|total| {
        if swap_over {
            total * constants::NEEDS_RAM_GROWTH_SWAP_OVER
        } else {
            total * constants::NEEDS_RAM_GROWTH_PRESSURE
        }
    });
    Some(Need {
        need: NeedKind::Ram,
        target: Some(target.name.clone()),
        platform: None,
        severity,
        summary: format!(
            "{} is short of memory: {} GiB available and swap at {}%",
            target.name,
            fmt(available),
            fmt(swap)
        ),
        evidence,
        suggestion: match suggested {
            Some(gb) => format!(
                "add memory to {} or replace it with a machine of about {} GiB",
                target.name,
                gb.round()
            ),
            None => format!("add memory to {}", target.name),
        },
    })
}

/// Free space below the declared target or low watermark.
fn storage_need(target: &ComputeTarget, payload: &Value, age: &str) -> Option<Need> {
    let free = diag_number(payload, "free_disk_gb")?;
    let policy = target.disk_cleanup.as_ref()?;
    let low = policy.low_free_gb as f64;
    let goal = policy.target_free_gb as f64;
    if free >= goal {
        return None;
    }
    let severity = if free < low {
        Severity::High
    } else {
        Severity::Medium
    };
    Some(Need {
        need: NeedKind::Storage,
        target: Some(target.name.clone()),
        platform: None,
        severity,
        summary: format!(
            "{} has {free:.1} GiB free against a declared target of {goal:.0} GiB",
            target.name
        ),
        evidence: vec![Evidence::new(
            "capacity",
            format!(
                "{} published {age}: free_disk_gb {free:.1}; registry targets[].disk_cleanup declares low {low:.0} GiB, target {goal:.0} GiB",
                target.name
            ),
        )],
        suggestion: format!(
            "add storage to {0} or run `stado space report {0}` and `stado space reclaim {0} --apply --reason <text>` to reclaim what its cleaners may take",
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
