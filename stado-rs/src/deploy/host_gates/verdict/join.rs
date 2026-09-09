//! The truth table: four sources in, one claiming verdict out.

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::payload::{accelerator_availability, diag_flag};
use crate::capabilities::{storage_reach, StorageReach};
use crate::deploy::host_disk;
use crate::deploy::host_gates::gates::HostGates;
use crate::deploy::host_gates::words::{
    AGENT_STORE_DEVICE_ONLY, AGENT_STORE_UNKNOWN, AGENT_STORE_UNREADABLE,
    CAPACITY_PUBLICATION_STALE, DISK_CLEANUP_LOCK_HELD, DISK_CLEANUP_POLICY_UNKNOWN,
    DISK_CLEANUP_STALLED, DISK_PRESSURE_UNRESOLVED, LOCAL_SNAPSHOTS_UNRECLAIMABLE,
    NO_CAPACITY_PUBLICATION, PINNED_ONLY, QUEUE_PAUSED, STALL_INTERVALS,
};
use crate::deploy::host_gates::DISK_PRESSURE_ACTIVE;
use crate::providers::local::disk_cleanup;
use crate::queue::capacity::{self, Publication};
use crate::targets::ComputeTarget;

/// Join the four sources into the verdict.
///
/// `agent_store` is the host's own effective `wc_storage_backend`, or `None`
/// when the host would not answer with one.
pub fn assemble(
    target: &ComputeTarget,
    reading: &host_disk::DiskReading,
    publication: Option<&Publication>,
    agent_store: Option<&str>,
    now: DateTime<Utc>,
    state_observed: bool,
    publication_observed: bool,
) -> HostGates {
    let policy = target.disk_cleanup.as_ref();
    let free_kb = reading
        .usage
        .as_ref()
        .and_then(|usage| usage.available_kb.parse::<u64>().ok());
    let free_bytes = free_kb.and_then(|blocks| blocks.checked_mul(1024));
    let free_gb = free_kb.map(|blocks| host_disk::gib_from_blocks(blocks as f64));
    // The registry's declared watermark first, and the janitor's state file
    // only where the registry declares no policy at all.
    //
    // This was the other way round, on the reasoning that the state file holds
    // the number the agent actually gated on and survives a registry the host
    // cannot read. Both halves are true and it still reported a number that
    // could not be acted on. That file is written by every cleanup pass, and on
    // an always-on host several processes make them: the queue agent every ten
    // seconds, a `disk-cleanup --watch` unit on its own timer, and any of them
    // may be a long-running process still holding a configuration that resolves
    // a superseded policy. On charless-mac-mini that produced `low watermark
    // 20 GiB, target 18 GiB` — a floor above its own ceiling, from a stale
    // 20/25 policy — alternating with the canonical 15/18 between one reading
    // and the next, while the registry said 15 throughout.
    //
    // So the declaration wins. It is what the fleet decided, this command has
    // just read it, and a watermark the operator cannot reconcile with the
    // policy document is worse than no watermark at all.
    let low_watermark_gb = policy.map(|policy| policy.low_free_gb).or_else(|| {
        reading
            .state
            .low_bytes
            .map(|bytes| bytes / disk_cleanup::GIB)
    });

    let payload = publication.map(|row| &row.payload);
    let published_at = payload
        .and_then(|payload| payload.get("published_at"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let age_seconds = publication
        .and_then(|row| row.stamp)
        .map(|stamp| (now - stamp).num_seconds());
    let stale = age_seconds.is_some_and(|age| age > capacity::CAPACITY_STALE_SECONDS as i64);
    let publication_current = age_seconds.is_some() && !stale;

    // The published verdict while the row is live — that IS the decision the
    // agent is making right now. Once the row is stale or absent the agent is
    // no longer talking, so the same function it uses is applied to the
    // numbers this command just measured itself.
    let published_pressure = diag_flag(payload, DISK_PRESSURE_UNRESOLVED);
    let disk_pressure_unresolved = match published_pressure {
        Some(published) if publication_current => published,
        _ => disk_cleanup::disk_pressure_unresolved(
            low_watermark_gb.map(|gb| gb * disk_cleanup::GIB),
            free_bytes.and_then(|bytes| i64::try_from(bytes).ok()),
        ),
    };
    let pressure_source = match published_pressure {
        Some(_) if publication_current => Some("capacity_publication"),
        _ if low_watermark_gb.is_some() && free_kb.is_some() => Some("host_disk_measurement"),
        _ => None,
    };

    // How late the janitor is against the interval IT declares, measured from
    // the last pass that actually completed. `last_success_at` and not
    // `last_pass_at`: the incident this exists for logged a pass every sixty
    // seconds for fifteen days, so "it ran recently" was true throughout and
    // meant nothing.
    let cleanup_success_age_seconds = reading
        .state
        .last_success_at
        .as_deref()
        .and_then(|stamp| DateTime::parse_from_rfc3339(&stamp.replace('Z', "+00:00")).ok())
        .map(|stamp| (now - stamp.with_timezone(&Utc)).num_seconds());
    // Armed only where the host declares a janitor that is supposed to run.
    // `mode: "off"` is a deliberate choice and never late, and a host with no
    // declared interval has nothing to be late against — that is
    // `disk_cleanup_policy_unknown`, which is already a blocker of its own.
    let stall_after_seconds = policy
        .filter(|policy| policy.mode != "off")
        .map(|policy| policy.check_interval_seconds * STALL_INTERVALS);
    // How long the janitor has been PREVENTED rather than silent. A workload
    // holds the run lock in shared mode for its whole duration, by design, and
    // every pass that starts meanwhile answers `lock_busy` — the modelled
    // answer, not a fault.
    let cleanup_prevented_age_seconds = reading
        .state
        .last_prevented_at
        .as_deref()
        .and_then(|stamp| DateTime::parse_from_rfc3339(&stamp.replace('Z', "+00:00")).ok())
        .map(|stamp| (now - stamp.with_timezone(&Utc)).num_seconds());
    // A pass prevented within the same window the stall is measured over is a
    // janitor that is still running and still being turned away, so the age of
    // its last success says nothing about its health. Only silence does.
    //
    // This is the whole of the 2026-09-03 false blocker: charless-mac-mini ran
    // one job for 42 minutes, the in-process janitor polled every ten seconds
    // throughout, and because a prevented pass recorded nothing the success age
    // reached 2311s against a 1200s limit and `claiming` went off — on a host
    // with 17.3 GiB free against a 15 GiB watermark and
    // `disk_pressure_unresolved: false`. The host was refusing new work because
    // it was doing work.
    let cleanup_prevented = match (stall_after_seconds, cleanup_prevented_age_seconds) {
        (Some(limit), Some(age)) => age <= limit,
        _ => false,
    };
    // Being turned away is healthy for as long as somebody is taking turns.
    // Being turned away while nothing has got through for the whole window the
    // stall is measured over is not being turned away — it is a lock that is
    // held, and it has a different remedy from every other condition here:
    // find the holder (`space report`'s `cleanup_lock.holders` names the pid) and
    // deal with THAT process. See [`DISK_CLEANUP_LOCK_HELD`].
    let disk_cleanup_lock_held = state_observed
        && cleanup_prevented
        && match (stall_after_seconds, cleanup_success_age_seconds) {
            (None, _) => false,
            (Some(_), None) => true,
            (Some(limit), Some(age)) => age > limit,
        };
    let disk_cleanup_stalled = state_observed
        && !cleanup_prevented
        && match (stall_after_seconds, cleanup_success_age_seconds) {
            (None, _) => false,
            // Declared, armed, and no completed pass on record at all. Reported
            // rather than excused: a janitor that has never finished a pass is
            // the fifteen-day case exactly, and the state file being absent or
            // fresh says nothing about whether the thing ever worked.
            (Some(_), None) => true,
            (Some(limit), Some(age)) => age > limit,
        };

    let mut blockers: Vec<String> = Vec::new();
    // First in the vector, ahead of the staleness it causes: an agent bound to
    // a device-local store cannot publish anything this control plane will
    // ever read, so its publication is missing or stale BY CONSTRUCTION, and
    // an operator reading `capacity_publication_stale` first goes looking at
    // the agent's uptime instead of at the config its unit exports.
    match agent_store.map(storage_reach) {
        Some(Some(StorageReach::Fleet)) | None => {}
        Some(Some(StorageReach::Device)) => blockers.push(AGENT_STORE_DEVICE_ONLY.to_string()),
        Some(None) => blockers.push(AGENT_STORE_UNKNOWN.to_string()),
    }
    if publication_observed && publication.is_none() {
        blockers.push(NO_CAPACITY_PUBLICATION.to_string());
    } else if stale {
        blockers.push(CAPACITY_PUBLICATION_STALE.to_string());
    }
    if publication_current && diag_flag(payload, DISK_PRESSURE_ACTIVE) == Some(true) {
        blockers.push(DISK_PRESSURE_ACTIVE.to_string());
    }
    if disk_pressure_unresolved {
        blockers.push(DISK_PRESSURE_UNRESOLVED.to_string());
    }
    // Directly after the pressure it explains: an operator who reads
    // "free 45 GiB, watermark 100 GiB" needs the next line to say whether
    // anything is still trying, and for fifteen days there was no such line.
    //
    // It blocks only while the disk is also under pressure, and that is the
    // case where a stalled janitor genuinely must refuse work: the host is
    // already below the watermark, nothing is bringing it back, and admitting
    // a job onto an unmanaged disk is how the fifteen-day incident ended. Above
    // the watermark it is a NOTE. Refusing work on a host with headroom does
    // not create a single byte of space; it only removes capacity from the
    // fleet, and it removed the always-on Mac from the fleet on 2026-09-03 over
    // a janitor that was healthy. The condition stays visible either way —
    // `disk_cleanup_stalled` is carried as a field and embedded in the release
    // verdict, so nothing that could see this before has stopped seeing it.
    if disk_cleanup_stalled && disk_pressure_unresolved {
        blockers.push(DISK_CLEANUP_STALLED.to_string());
    }
    if disk_cleanup_lock_held && disk_pressure_unresolved {
        blockers.push(DISK_CLEANUP_LOCK_HELD.to_string());
    }
    if diag_flag(payload, "disk_cleanup_policy_known") == Some(false) || low_watermark_gb.is_none()
    {
        blockers.push(DISK_CLEANUP_POLICY_UNKNOWN.to_string());
    }
    if diag_flag(payload, QUEUE_PAUSED) == Some(true) {
        blockers.push(QUEUE_PAUSED.to_string());
    }

    // The note fires only while the disk is the reason this host claims
    // nothing: snapshots on a healthy box are a backup policy, not a finding,
    // and a command that reports them every time is a command whose output
    // stops being read.
    let local_snapshots = reading
        .snapshots
        .supported
        .then_some(reading.snapshots.names.len());
    let mut notes: Vec<String> = Vec::new();
    if diag_flag(payload, PINNED_ONLY) == Some(true) || target.pinned_only {
        notes.push(PINNED_ONLY.to_string());
    }
    if disk_pressure_unresolved && local_snapshots.is_some_and(|count| count > 0) {
        notes.push(LOCAL_SNAPSHOTS_UNRECLAIMABLE.to_string());
    }
    // A janitor that is late on a host that still has its headroom. Not a
    // blocker (see the pressure gate above) and not silence either: an
    // operator has to be told that the mechanism which maintains this host's
    // free space is not running, before the day it matters.
    if disk_cleanup_stalled && !disk_pressure_unresolved {
        notes.push(DISK_CLEANUP_STALLED.to_string());
    }
    // The same finding for a lock that is held rather than a janitor that is
    // silent, and a note for the same reason: a host with headroom that cannot
    // clean is a host to go fix, not a host to close.
    if disk_cleanup_lock_held && !disk_pressure_unresolved {
        notes.push(DISK_CLEANUP_LOCK_HELD.to_string());
    }
    if agent_store.is_none() {
        notes.push(AGENT_STORE_UNREADABLE.to_string());
    }

    HostGates {
        host: target.name.clone(),
        claiming: blockers.is_empty(),
        blockers,
        disk_pressure_unresolved,
        free_bytes,
        disk_cleanup_stalled,
        disk_cleanup_lock_held,
        cleanup_success_age_seconds,
        cleanup_prevented_age_seconds,
        free_gb,
        low_watermark_gb,
        target_free_gb: policy.map(|policy| policy.target_free_gb),
        policy_mode: policy.map(|policy| policy.mode.clone()),
        published_at,
        age_seconds,
        accepting_jobs: payload
            .and_then(|value| value.get("accepting_jobs"))
            .and_then(Value::as_bool),
        running_jobs: payload
            .and_then(|value| value.get("running_jobs"))
            .and_then(Value::as_i64),
        available_cpu_cores: payload
            .and_then(|value| value.get("available_cpu_cores"))
            .and_then(Value::as_i64),
        total_cpu_cores: payload
            .and_then(|value| value.get("total_cpu_cores"))
            .and_then(Value::as_i64),
        available_accelerators: payload.map(accelerator_availability).unwrap_or_default(),
        free_ram_gb: payload
            .and_then(|value| value.get("free_ram_gb"))
            .and_then(Value::as_f64),
        total_ram_gb: payload
            .and_then(|value| value.get("total_ram_gb"))
            .and_then(Value::as_f64),
        free_vram_gb: payload
            .and_then(|value| value.get("free_vram_gb"))
            .and_then(Value::as_i64),
        total_vram_gb: payload
            .and_then(|value| value.get("total_vram_gb"))
            .and_then(Value::as_i64),
        agent_store_backend: agent_store.map(str::to_string),
        fleet_store_backend: crate::config::wc_storage_backend().to_string(),
        notes,
        local_snapshots,
        waiting_jobs: Vec::new(),
        complete: true,
        observations: Vec::new(),
        pressure_source,
        published_diagnostics: payload.and_then(|value| value.get("diag")).cloned(),
    }
}
