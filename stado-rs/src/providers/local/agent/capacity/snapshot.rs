//! The measured capacity document and the broadcast that carries it.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::providers::local::disk::gate::DiskGateDiag;
use crate::providers::local::helpers;
use crate::providers::local::slots::ActiveSlot;
use crate::queue::capacity::{publish_capacity, CapacitySnapshot};
use crate::queue::{JobStorage, StorageError};

/// The disk gate's figures as published: each measured value under its name,
/// null when it was not measured, with the cause beside it under
/// `<name>_unread`, so a reader is told why rather than shown a gap.
pub(crate) fn diag_map(d: &DiskGateDiag) -> Vec<(String, Value)> {
    let measured = |name: &str, reading: &Result<f64, String>| {
        let (value, unread) = match reading {
            Ok(value) => (Value::from(*value), Value::Null),
            Err(cause) => (Value::Null, Value::from(cause.as_str())),
        };
        [(name.to_string(), value), (format!("{name}_unread"), unread)]
    };
    let mut map: Vec<(String, Value)> = Vec::new();
    map.extend(measured("free_disk_gb", &d.free_disk_gb));
    map.push(("home_write_probe_ok".to_string(), Value::from(d.home_write_probe.is_ok())));
    map.push((
        "home_write_probe_error".to_string(),
        match &d.home_write_probe {
            Ok(()) => Value::Null,
            Err(cause) => Value::from(cause.as_str()),
        },
    ));
    map.extend(measured("staging_free_gb", &d.staging_free_gb));
    let (pending, unread) = match &d.largest_pending_raw_dir_gb {
        Ok(value) => (Value::from(*value), Value::Null),
        Err(cause) => (Value::Null, Value::from(cause.as_str())),
    };
    map.push(("largest_pending_raw_dir_gb".to_string(), pending));
    map.push(("largest_pending_raw_dir_gb_unread".to_string(), unread));
    map
}
pub(crate) fn measured_capacity(
    running: &[ActiveSlot],
    policy_allows_jobs: bool,
    policy_reason: Option<&str>,
    available_accelerators: BTreeMap<String, i64>,
    free_vram_gb: Option<i64>,
    total_vram_gb: i64,
    mut diag: Map<String, Value>,
) -> CapacitySnapshot {
    let requested_cpu = running
        .iter()
        .map(|active| helpers::requested_cpu_cores(&active.slot.job))
        .sum();
    // An unreadable core count or processor-time counter publishes no CPU
    // figures, and the operating system's refusal is published below as the
    // admission detail; the first sample is a baseline and says so.
    let (total_cpu_cores, available_cpu_cores, cpu_unmeasured) =
        match (helpers::total_cpu_cores(), helpers::available_cpu_cores(requested_cpu)) {
            (Ok(total), Ok(Some(available))) => (Some(total), Some(available), None),
            (Ok(total), Ok(None)) => (
                Some(total),
                None,
                Some("the first processor-time sample is a baseline; the next pass measures CPU".to_string()),
            ),
            (Err(cause), _) | (Ok(_), Err(cause)) => (None, None, Some(cause)),
        };
    // An unreadable memory measurement publishes no RAM figures, and its cause
    // is published below as the admission detail.
    let (free_ram_gb, total_ram_gb, memory_unread) = match helpers::memory_gb() {
        Ok((free, total)) => (Some(free), Some(total), None),
        Err(cause) => (None, None, Some(cause)),
    };
    // No reserve is held back from what was measured: a value that decides
    // admission needs a source, and none stated how much to keep free. A job
    // is admitted against the measured free RAM and VRAM, in which every
    // running slot already counts as the larger of its declaration, its live
    // use and its measured peak.
    let exclusive_running = running
        .iter()
        .any(|active| helpers::slot_is_exclusive(&active.slot));
    // `memory_measurement_unavailable` alone told nobody why, so the
    // operating system's own refusal is published as `admission_detail`,
    // which `stado host gates` and Stado Desktop already print beside it.
    let mut resource_detail: Option<String> = None;
    let resource_reason = if !policy_allows_jobs {
        policy_reason
    } else if exclusive_running {
        Some("exclusive_job_running")
    } else if let Some(cause) = &cpu_unmeasured {
        resource_detail = Some(cause.clone());
        Some("cpu_measurement_unavailable")
    } else if available_cpu_cores.is_some_and(|cores| !cores.is_positive()) {
        Some("cpu_busy")
    } else if let Some(cause) = &memory_unread {
        resource_detail = Some(cause.clone());
        Some("memory_measurement_unavailable")
    } else if free_ram_gb.is_some_and(|free| !free.is_normal() || free.is_sign_negative()) {
        Some("ram_headroom_low")
    } else {
        None
    };
    if let Some(reason) = resource_reason {
        diag.insert("admission_reason".into(), Value::from(reason));
    } else {
        diag.remove("admission_reason");
    }
    match resource_detail {
        Some(detail) => {
            diag.insert("admission_detail".into(), Value::from(detail));
        }
        None => {
            diag.remove("admission_detail");
        }
    }
    // An unreadable load average is published as null, not as an idle host.
    diag.insert("cpu_load_1m".into(), Value::from(helpers::load_average_1m()));
    CapacitySnapshot {
        accepting_jobs: resource_reason.is_none(),
        running_jobs: running.len(),
        total_cpu_cores,
        available_cpu_cores,
        available_accelerators,
        free_ram_gb,
        total_ram_gb,
        free_vram_gb,
        total_vram_gb,
        diag,
    }
}

/// Publish one capacity broadcast and say, in the log, which branch of the loop
/// produced it and whether the store accepted it.
///
/// The loop has several exit paths. A space-reclaim report alone says
/// nothing about whether the next capacity publication reached the store;
/// each branch therefore records its own result and failed write.
///
/// Two of the publish sites also discarded the store's answer with `let _ =`, so
/// an agent whose every write was being refused reported exactly what an agent
/// with nothing to say reports. The result is returned to the caller, and the
/// failure is logged here either way, because a broadcast nobody accepted is the
/// one event this process exists to perform.
pub(crate) async fn publish_branch(
    store: &JobStorage,
    consumer_id: &str,
    kind: &str,
    branch: &str,
    snapshot: &CapacitySnapshot,
    log_fn: &mut dyn FnMut(&str),
) -> Result<(), StorageError> {
    let outcome = publish_capacity(store, consumer_id, kind, snapshot).await;
    match &outcome {
        Ok(()) => log_fn(&format!(
            "loop: {branch}: published accepting_jobs={} running_jobs={} \
             available_cpu_cores={} free_vram_gb={}",
            snapshot.accepting_jobs,
            snapshot.running_jobs,
            snapshot.available_cpu_shown(),
            snapshot.free_vram_shown()
        )),
        Err(exc) => log_fn(&format!(
            "loop: {branch}: capacity publish REFUSED by the store: {exc}"
        )),
    }
    outcome
}
