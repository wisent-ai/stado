//! The measured capacity document and the broadcast that carries it.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::constants;
use crate::providers::local::disk_gate::DiskGateDiag;
use crate::providers::local::helpers;
use crate::providers::local::slots::ActiveSlot;
use crate::queue::capacity::{publish_capacity, CapacitySnapshot};
use crate::queue::{JobStorage, StorageError};

pub(crate) fn diag_map(d: &DiskGateDiag) -> [(String, Value); 4] {
    [
        ("free_disk_gb".to_string(), Value::from(d.free_disk_gb)),
        (
            "home_write_probe_ok".to_string(),
            Value::from(d.home_write_probe_ok),
        ),
        (
            "staging_free_gb".to_string(),
            Value::from(d.staging_free_gb),
        ),
        (
            "largest_pending_raw_dir_gb".to_string(),
            Value::from(d.largest_pending_raw_dir_gb),
        ),
    ]
}
pub(crate) fn measured_capacity(
    running: &[ActiveSlot],
    policy_allows_jobs: bool,
    policy_reason: Option<&str>,
    available_accelerators: BTreeMap<String, i64>,
    free_vram_gb: i64,
    total_vram_gb: i64,
    mut diag: Map<String, Value>,
) -> CapacitySnapshot {
    let requested_cpu = running
        .iter()
        .map(|active| helpers::requested_cpu_cores(&active.slot.job))
        .sum();
    let total_cpu_cores = helpers::total_cpu_cores();
    let measured_cpu_cores = helpers::available_cpu_cores(requested_cpu);
    let available_cpu_cores = measured_cpu_cores.unwrap_or(0);
    let memory = helpers::memory_gb();
    let free_ram_gb = memory.map(|(free, _)| free);
    let total_ram_gb = memory.map(|(_, total)| total);
    let ram_reserve_gb = total_ram_gb
        .map(|total| {
            (constants::RAM_SAFETY_BUFFER_MIN_GB as f64)
                .max(total * constants::RAM_SAFETY_BUFFER_FRACTION)
        })
        .unwrap_or(constants::RAM_SAFETY_BUFFER_MIN_GB as f64);
    let exclusive_running = running
        .iter()
        .any(|active| helpers::slot_is_exclusive(&active.slot));
    let resource_reason = if !policy_allows_jobs {
        policy_reason
    } else if exclusive_running {
        Some("exclusive_job_running")
    } else if measured_cpu_cores.is_none() {
        Some("cpu_measurement_unavailable")
    } else if available_cpu_cores == 0 {
        Some("cpu_busy")
    } else if free_ram_gb.is_none() {
        Some("memory_measurement_unavailable")
    } else if free_ram_gb.is_some_and(|free| free < ram_reserve_gb + 1.0) {
        Some("ram_headroom_low")
    } else {
        None
    };
    if let Some(reason) = resource_reason {
        diag.insert("admission_reason".into(), Value::from(reason));
    } else {
        diag.remove("admission_reason");
    }
    diag.insert(
        "cpu_load_1m".into(),
        helpers::load_average_1m().map_or(Value::Null, Value::from),
    );
    diag.insert("ram_safety_buffer_gb".into(), Value::from(ram_reserve_gb));
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
/// The loop below can leave its iteration nine ways and used to name none of
/// them. On the always-on mac that cost seven days: the log showed
/// `loop: iter-start` and a disk-cleanup report every ten seconds, nothing
/// after, and the broadcast in the fleet store stayed frozen at a timestamp
/// three minutes before the agent's unit was re-declared. Both facts were
/// consistent with about four different branches and with a crash-restart loop,
/// and separating them took a census of a log that should simply have said.
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
            snapshot.available_cpu_cores,
            snapshot.free_vram_gb
        )),
        Err(exc) => log_fn(&format!(
            "loop: {branch}: capacity publish REFUSED by the store: {exc}"
        )),
    }
    outcome
}
