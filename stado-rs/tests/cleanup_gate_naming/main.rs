//! A gate that refuses work has to name the condition it refused for.
//!
//! # What happened
//!
//! On 2026-09-03 `disk_cleanup_stalled` closed every `darwin-arm64` builder in
//! the registry at once: `charless-mac-mini` at 18.4 GiB free against a 15 GiB
//! low watermark with eight jobs pinned to it, and `lukasz-macbook` at 118.7
//! GiB free against 100 with one job that had waited a day. Neither disk was
//! under any pressure. Both janitors were stale for one reason — a shared
//! workload hold that had outlived its workload made every exclusive acquire
//! fail, so no pass ever scanned — and the word the operator was handed sent
//! them to a disk that was fine.
//!
//! # What is defended here
//!
//! Three separable properties of `host_gates::assemble`, which is the whole
//! truth table and is exercised here with no host, no registry and no store:
//!
//! 1. A janitor that cannot get the run lock is `disk_cleanup_lock_held` and
//!    NOT `disk_cleanup_stalled`. The two are mutually exclusive and name
//!    different remedies: one is a process holding a file, the other is a
//!    janitor that ran and got nowhere.
//! 2. Neither condition closes a host that measurably has its headroom. Above
//!    the watermark they are notes; refusing work there frees no byte and it
//!    took a platform offline.
//! 3. Under pressure both still refuse. That is the risk the gate exists for
//!    and it is not weakened here.

use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use stado::deploy::host_disk::{CleanupState, DiskReading, DiskUsage};
use stado::deploy::host_gates::{
    assemble, DISK_CLEANUP_LOCK_HELD, DISK_CLEANUP_STALLED, DISK_PRESSURE_UNRESOLVED,
};
use stado::targets::ComputeTarget;

/// The declared interval. `STALL_INTERVALS` is 4, so the stall window is
/// 1200s — the same 300s policy charless-mac-mini declares.
const INTERVAL_SECONDS: i64 = 300;
/// Comfortably outside the stall window.
const STALE_SECONDS: i64 = 4000;

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-09-03T18:00:00Z")
        .expect("static timestamp")
        .with_timezone(&Utc)
}

fn stamp(seconds_ago: i64) -> String {
    (now() - Duration::seconds(seconds_ago)).to_rfc3339()
}

/// A host declaring an armed janitor with a 15 GiB low watermark, exactly as
/// the registry declares charless-mac-mini.
fn target() -> ComputeTarget {
    serde_json::from_value(json!({
        "name": "t1",
        "kind": "local",
        "ssh": "u@10.0.0.1",
        "release_platform": "darwin-arm64",
        "hostnames": ["t1.local"],
        "disk_cleanup": {
            "mode": "enforce",
            "check_interval_seconds": INTERVAL_SECONDS,
            "low_free_gb": 15,
            "target_free_gb": 18,
            "max_bytes_per_pass": 64 * 1024_i64.pow(3),
            "max_items_per_pass": 512,
            "max_scan_items": 4096,
            "cleaners": { "build_caches": { "min_age_seconds": 86400 } }
        }
    }))
    .expect("the fixture target deserializes")
}

/// `df -Pk` available blocks for `free_gib` GiB.
fn reading(free_gib: i64, state: CleanupState) -> DiskReading {
    DiskReading {
        usage: Some(DiskUsage {
            available_kb: (free_gib * 1024 * 1024).to_string(),
            ..DiskUsage::default()
        }),
        state,
        ..DiskReading::default()
    }
}

/// A janitor whose last success is outside the stall window and which is being
/// refused the lock on every tick — the wedge.
fn lock_held_state() -> CleanupState {
    CleanupState {
        present: true,
        outcome: Some("lock_busy".to_string()),
        last_success_at: Some(stamp(STALE_SECONDS)),
        // The agent polls every ten seconds, so the newest prevented pass is
        // always seconds old while the wedge lasts.
        last_prevented_at: Some(stamp(5)),
        ..CleanupState::default()
    }
}

/// A janitor that is simply silent: stale success, and nothing recording that
/// anything was turned away.
fn silent_state() -> CleanupState {
    CleanupState {
        present: true,
        last_success_at: Some(stamp(STALE_SECONDS)),
        last_prevented_at: None,
        ..CleanupState::default()
    }
}

/// The fixture must produce a claiming host when the janitor is healthy, or
/// every refusal below could be the fixture's fault.
#[test]
fn a_healthy_janitor_with_headroom_claims() {
    let state = CleanupState {
        present: true,
        last_success_at: Some(stamp(30)),
        ..CleanupState::default()
    };
    let gates = assemble(&target(), &reading(18, state), None, Some("stado"), now());
    assert!(!gates.disk_cleanup_stalled, "a fresh pass is not stale");
    assert!(!gates.disk_cleanup_lock_held, "nothing is holding the lock");
    assert!(
        !gates.blockers.iter().any(|b| b == DISK_CLEANUP_STALLED
            || b == DISK_CLEANUP_LOCK_HELD
            || b == DISK_PRESSURE_UNRESOLVED),
        "no cleanup blocker on a healthy host: {:?}",
        gates.blockers
    );
}

/// Property 1: the wedge is named as a held lock, and is NOT reported as a
/// stalled janitor. Before this split the wedge produced either the stalled
/// word (which points at the disk) or, once prevented passes began recording
/// themselves, nothing at all.
#[test]
fn a_held_run_lock_is_named_as_a_held_lock() {
    let gates = assemble(
        &target(),
        &reading(18, lock_held_state()),
        None,
        Some("stado"),
        now(),
    );
    assert!(
        gates.disk_cleanup_lock_held,
        "a janitor refused the lock with no success inside the window is a held lock"
    );
    assert!(
        !gates.disk_cleanup_stalled,
        "a held lock must not also be reported as a stalled janitor: one remedy each"
    );
    assert_eq!(
        gates.cleanup_prevented_age_seconds,
        Some(5),
        "the number behind the word has to travel with it"
    );
    assert!(
        gates
            .notes
            .iter()
            .any(|note| note == DISK_CLEANUP_LOCK_HELD),
        "the condition must be reported: {:?}",
        gates.notes
    );
    assert!(
        !gates.notes.iter().any(|note| note == DISK_CLEANUP_STALLED),
        "the disk-shaped word must not appear for a lock-shaped fault: {:?}",
        gates.notes
    );
}

/// A silent janitor keeps the word it already had. The split must not have
/// renamed the condition it was right about.
#[test]
fn a_silent_janitor_is_still_named_as_stalled() {
    let gates = assemble(
        &target(),
        &reading(18, silent_state()),
        None,
        Some("stado"),
        now(),
    );
    assert!(gates.disk_cleanup_stalled, "a silent janitor is stalled");
    assert!(
        !gates.disk_cleanup_lock_held,
        "nothing recorded a prevented pass, so nothing is holding the lock"
    );
}

/// Property 2: measured headroom decides. Both hosts that went offline had
/// theirs, and a gate that refuses work above its own watermark frees no byte
/// — it only removes the platform's last builder.
#[test]
fn neither_condition_closes_a_host_that_has_its_headroom() {
    for (label, state) in [("lock_held", lock_held_state()), ("silent", silent_state())] {
        let gates = assemble(&target(), &reading(18, state), None, Some("stado"), now());
        assert!(
            !gates.disk_pressure_unresolved,
            "{label}: 18 GiB against a 15 GiB watermark is not pressure"
        );
        assert!(
            !gates
                .blockers
                .iter()
                .any(|b| b == DISK_CLEANUP_STALLED || b == DISK_CLEANUP_LOCK_HELD),
            "{label}: cleanup must not block a host with headroom: {:?}",
            gates.blockers
        );
    }
}

/// Property 3: the safety property is untouched. Below the watermark a janitor
/// that cannot run must still refuse work — nothing is bringing the space
/// back, and admitting a job onto an unmanaged disk is the incident this gate
/// was written for.
#[test]
fn under_pressure_a_janitor_that_cannot_run_still_refuses_work() {
    for (label, state, expected) in [
        ("lock_held", lock_held_state(), DISK_CLEANUP_LOCK_HELD),
        ("silent", silent_state(), DISK_CLEANUP_STALLED),
    ] {
        let gates = assemble(&target(), &reading(9, state), None, Some("stado"), now());
        assert!(
            gates.disk_pressure_unresolved,
            "{label}: 9 GiB against a 15 GiB watermark is pressure"
        );
        assert!(
            gates.blockers.iter().any(|blocker| blocker == expected),
            "{label}: expected {expected} in {:?}",
            gates.blockers
        );
        assert!(
            !gates.claiming,
            "{label}: a host under pressure with a janitor that cannot run must not claim"
        );
    }
}
