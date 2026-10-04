//! The two payloads the verdict is published in: the operator console's
//! `--json` document and the slice a release verdict embeds.

use serde_json::{json, Map, Value};

use crate::deploy::host_gates::gates::HostGates;

/// The `--json` report, in the exact shape the operator console consumes.
pub fn to_report(gates: &HostGates) -> Map<String, Value> {
    let mut report = Map::new();
    let disk_read = gates
        .observations
        .iter()
        .find(|read| read.operation == "disk_usage");
    let state_read = gates
        .observations
        .iter()
        .find(|read| read.operation == "host_state");
    let queue_read = gates
        .observations
        .iter()
        .find(|read| read.operation == "queue");
    let state_known = state_read.is_some_and(|read| read.complete());
    report.insert("complete".to_string(), json!(gates.complete));
    report.insert("observations".to_string(), json!(gates.observations));
    report.insert("host".to_string(), Value::String(gates.host.clone()));
    report.insert(
        "claiming".to_string(),
        json!(gates.complete.then_some(gates.claiming)),
    );
    report.insert(
        "blockers".to_string(),
        Value::Array(
            gates
                .blockers
                .iter()
                .map(|blocker| Value::String(blocker.clone()))
                .collect(),
        ),
    );
    report.insert(
        "notes".to_string(),
        Value::Array(
            gates
                .notes
                .iter()
                .map(|note| Value::String(note.clone()))
                .collect(),
        ),
    );
    report.insert(
        "disk".to_string(),
        json!({
            "free_bytes": gates.free_bytes,
            "observed_at": disk_read.filter(|read| read.complete()).map(|read| &read.finished_at),
            "read_state": disk_read.map(|read| read.state),
            "pressure_source": gates.pressure_source,
            "pressure_unresolved": gates.pressure_source.map(|_| gates.disk_pressure_unresolved),
            "free_gb": gates.free_gb,
            "used_percent": gates.used_percent,
            "headroom_gb": gates.headroom_gb,
            "full_percent": crate::providers::local::disk_cleanup::rule::DISK_FULL_PERCENT,
            "full": gates.used_percent.map(|used| {
                used >= f64::from(crate::providers::local::disk_cleanup::rule::DISK_FULL_PERCENT)
            }),
            // Snapshot space is reported separately: on a Mac deleted bytes
            // stay pinned until local Time Machine snapshots are thinned.
            // Null where unsupported.
            "local_snapshots": gates.local_snapshots,
            // Storage the host has and the fleet cannot reach: disks
            // attached and unmounted, each named with its size.
            "unmounted_disks": gates.unmounted_disks,
            // Whether anything is still trying to keep the two numbers above
            // apart, and how long since it last managed to.
            "cleanup_stalled": state_known.then_some(gates.disk_cleanup_stalled),
            "cleanup_success_age_seconds": gates.cleanup_success_age_seconds,
            // ...and whether it is not trying because it cannot get the lock,
            // which points at a process and not at this disk.
            "cleanup_lock_held": state_known.then_some(gates.disk_cleanup_lock_held),
            "cleanup_prevented_age_seconds": gates.cleanup_prevented_age_seconds,
        }),
    );
    report.insert("memory".to_string(), gates.memory.to_value());
    report.insert(
        "capacity".to_string(),
        json!({
            "published_at": gates.published_at,
            "diagnostics": gates.published_diagnostics,
            "age_seconds": gates.age_seconds,
            "accepting_jobs": gates.accepting_jobs,
            "admission_reason": gates.admission_reason,
            "running_jobs": gates.running_jobs,
            "running_workloads": gates.running_workloads,
            "reserved": gates.reserved,
            "reservations": gates.reservations,
            "available_cpu_cores": gates.available_cpu_cores,
            "total_cpu_cores": gates.total_cpu_cores,
            "available_accelerators": gates.available_accelerators,
            "free_ram_gb": gates.free_ram_gb,
            "total_ram_gb": gates.total_ram_gb,
            "free_vram_gb": gates.free_vram_gb,
            "total_vram_gb": gates.total_vram_gb,
        }),
    );
    report.insert(
        "store".to_string(),
        json!({
            // Both ends of the sentence, never a boolean verdict: the operator
            // who has to go fix the unit needs the backend name the host
            // resolved, and the one this control plane reads, side by side.
            // `agent_backend` is null on a host that would not answer, which
            // the `agent_store_unreadable` note also says.
            "agent_backend": gates.agent_store_backend,
            "fleet_backend": gates.fleet_store_backend,
        }),
    );
    report.insert(
        "waiting_jobs".to_string(),
        Value::Array(
            gates
                .waiting_jobs
                .iter()
                .map(|job| {
                    json!({
                        "job_id": job.job_id,
                        "age_seconds": job.age_seconds,
                        "declined": job.declined,
                    })
                })
                .collect(),
        ),
    );
    if queue_read.is_none_or(|read| !read.complete()) {
        report.insert("waiting_jobs".to_string(), Value::Null);
    }
    report
}

/// The fields a release verdict embeds when it has to say why a host is not
/// building anything.
///
/// Exported so `stado release doctor` reports the claiming gates from this
/// reader instead of growing a second one: two readers of
/// `capacity/<consumer>.json` would eventually give two answers to one
/// question, and the operator would believe whichever they ran first.
///
/// `disk_cleanup_stalled` rides here beside the pressure and the two numbers
/// because a release verdict is where this fleet actually looks. A host that
/// stops every release can report the pressure and the numbers correctly for
/// days; what no verdict anywhere says is that the janitor which is supposed
/// to resolve them has not completed a pass in weeks.
pub fn gates_section(gates: &HostGates) -> Value {
    let state_known = gates
        .observations
        .iter()
        .any(|read| read.operation == "host_state" && read.complete());
    json!({
        "complete": gates.complete,
        "observations": gates.observations,
        "pressure_source": gates.pressure_source,
        "disk_pressure_unresolved": gates.pressure_source.map(|_| gates.disk_pressure_unresolved),
        "disk_cleanup_stalled": state_known.then_some(gates.disk_cleanup_stalled),
        "disk_cleanup_lock_held": state_known.then_some(gates.disk_cleanup_lock_held),
        "cleanup_success_age_seconds": gates.cleanup_success_age_seconds,
        "free_bytes": gates.free_bytes,
        "free_gb": gates.free_gb,
        "used_percent": gates.used_percent,
        "headroom_gb": gates.headroom_gb,
        // The memory readings ride here too: a release verdict is where this
        // fleet actually looks, and a builder that stops publishing is most
        // often one that has run out of memory.
        "memory_available_gb": gates.memory.available_gb,
        "memory_swap_used_pct": gates.memory.swap_used_pct,
    })
}
