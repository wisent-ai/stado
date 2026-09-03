//! A release builder must be able to CLAIM the work, not merely to talk.
//!
//! # The incident
//!
//! `cli::release_submit::builder` selected purely on the presence of a live
//! capacity publication, and the host it picks is written into `pinned_host`,
//! where it is fixed for the job's lifetime. On 2026-09-03 that pinned release
//! run 987591db to charless-mac-mini at 16:59Z. The host was publishing every
//! ~21 seconds, comfortably inside the 180s staleness cutoff — and its gates
//! refused every job: `charless-mac-mini is claiming nothing:
//! disk_cleanup_stalled`, with 18.4 GiB free against a 15 GiB watermark. The
//! queue does not re-choose, it honours a pin already written, so the build job
//! sat unclaimed indefinitely while the submit reported nothing at all.
//! Publishing and claiming are two different facts and only one was checked.
//!
//! The other declared darwin-arm64 target was blocked by the same gate at the
//! same moment, so the platform had no builder — and a submit still succeeded
//! in pinning one.
//!
//! # What is defended
//!
//! * a publication with free slots is claimable;
//! * a publication that declares a slot table with nothing free is a refusal,
//!   and is never pinned — it is the shape a gate-blocked agent broadcasts;
//! * the refusal carries the blockers the host itself declared, in the same
//!   words `deploy::host_gates` uses, so one host reads identically in both
//!   commands;
//! * a publication with no slot table at all is NOT a refusal. That is the
//!   half that keeps this from becoming the opposite defect: a selector that
//!   refuses whatever it cannot read would take the fleet offline just as
//!   thoroughly as the old one let jobs queue forever. Silence is not "no".
//!
//! These are library-level tests over the judgement itself, which takes the
//! publication as data — so the whole table is exercised without a store, a
//! host, an SSH channel or a queue.

use serde_json::{json, Value};

use stado::cli::release_submit::{claimability, Claimability};
use stado::deploy::host_gates::{
    DISK_CLEANUP_POLICY_UNKNOWN, DISK_CLEANUP_STALLED, DISK_PRESSURE_UNRESOLVED, QUEUE_PAUSED,
};

/// A publication shaped like the ones this fleet actually writes.
fn publication(free_slots: Option<Value>, diag: Value) -> Value {
    let mut payload = json!({
        "consumer_id": "local-Charless-Mac-mini.local",
        "kind": "local",
        "published_at": "2026-09-03T17:42:17.379737+00:00",
        "diag": diag,
    });
    if let Some(slots) = free_slots {
        payload["free_slots"] = slots;
    }
    payload
}

/// A host with capacity is selectable, and says how much.
#[test]
fn a_host_with_free_slots_is_claimable() {
    let verdict = claimability(&publication(
        Some(json!({"cpu": 1, "nvidia-l4": 0})),
        json!({}),
    ));
    assert_eq!(verdict, Claimability::Claimable { free_slots: 1 });
    assert!(verdict.eligible());
    assert!(verdict.describe().contains("1 free slot"));
}

/// The incident, as an invariant. charless-mac-mini published exactly this —
/// an empty slot table — while publishing punctually every ~21 seconds. It
/// must never be pinned again.
#[test]
fn a_host_publishing_no_free_slots_is_refused_not_pinned() {
    let verdict = claimability(&publication(Some(json!({})), json!({})));
    assert_eq!(
        verdict,
        Claimability::Refusing {
            blockers: Vec::new()
        }
    );
    assert!(
        !verdict.eligible(),
        "a host that will take nothing must not receive an irrevocable pin"
    );
}

/// A slot table that is present but exhausted is the same refusal: every
/// declared slot taken is still "this host will accept nothing now".
#[test]
fn a_host_whose_every_slot_is_taken_is_refused() {
    let verdict = claimability(&publication(Some(json!({"cpu": 0, "gpu": 0})), json!({})));
    assert!(!verdict.eligible());
}

/// The refusal has to be legible, which is the whole point of the change: the
/// caller learns which host was considered and what stopped it, in the words
/// `host gates` already uses.
#[test]
fn a_refusal_carries_the_blockers_the_host_declared() {
    let verdict = claimability(&publication(
        Some(json!({})),
        json!({
            "disk_pressure_unresolved": true,
            "disk_cleanup_policy_known": false,
            "queue_paused": true,
            "disk_cleanup": {"outcome": "lock_busy", "lock_busy": true},
        }),
    ));
    let Claimability::Refusing { blockers } = &verdict else {
        panic!("expected a refusal, got {verdict:?}");
    };
    assert!(blockers.iter().any(|b| b == DISK_PRESSURE_UNRESOLVED));
    assert!(blockers.iter().any(|b| b == DISK_CLEANUP_POLICY_UNKNOWN));
    assert!(blockers.iter().any(|b| b == QUEUE_PAUSED));
    assert!(
        blockers.iter().any(|b| b.starts_with(DISK_CLEANUP_STALLED)),
        "a lock_busy pass never advances last_success_at, which is what closed \
         both darwin-arm64 builders: {blockers:?}"
    );
    let described = verdict.describe();
    assert!(described.contains("0 free slot(s)"), "{described}");
    assert!(described.contains(DISK_CLEANUP_STALLED), "{described}");
}

/// The exact publication the wedged mini was broadcasting: punctual, fresh,
/// empty slot table, janitor pass exiting `lock_busy`. Refused, and the reason
/// names the janitor.
#[test]
fn the_wedged_builders_own_publication_is_refused_with_its_reason() {
    let verdict = claimability(&json!({
        "consumer_id": "local-Charless-Mac-mini.local",
        "kind": "local",
        "free_slots": {},
        "free_vram_gb": 1,
        "total_vram_gb": 1,
        "published_at": "2026-09-03T17:42:17.379737+00:00",
        "diag": {
            "storage_backend": "stado",
            "storage_answers_for_fleet": true,
            "disk_cleanup": {
                "writer": "agent-tick",
                "writer_version": "0.13.52",
                "writer_pid": 79473,
                "outcome": "lock_busy",
                "lock_busy": true,
                "duration_ms": 372,
                "last_success_at": Value::Null,
            },
        },
    }));
    assert!(!verdict.eligible());
    assert!(verdict.describe().contains(DISK_CLEANUP_STALLED));
}

/// The guard against becoming the opposite defect. A publication with no slot
/// table has not refused anything, and must stay eligible: refusing on
/// unreadable data would take every builder offline the moment a field went
/// missing, which is a worse outage than the one being fixed.
#[test]
fn a_publication_with_no_slot_table_is_not_a_refusal() {
    let verdict = claimability(&publication(None, json!({"storage_backend": "stado"})));
    assert_eq!(verdict, Claimability::Unstated);
    assert!(
        verdict.eligible(),
        "silence from a host is not the host saying no; inventing a refusal from a missing \
         field is how a selector takes a fleet offline"
    );
    assert!(verdict.describe().contains("no slot table"));
}

/// A slot table of the wrong shape is the same silence, not a refusal.
#[test]
fn an_unreadable_slot_table_is_treated_as_silence_not_refusal() {
    for shape in [json!("plenty"), json!(3), json!([1, 2]), Value::Null] {
        let verdict = claimability(&publication(Some(shape.clone()), json!({})));
        assert_eq!(verdict, Claimability::Unstated, "shape {shape:?}");
        assert!(verdict.eligible(), "shape {shape:?}");
    }
}

/// Non-numeric slot values must not be counted as capacity, but must not
/// manufacture capacity either: a table whose values cannot be summed to a
/// positive number is a refusal, not a free slot.
#[test]
fn slot_values_that_are_not_numbers_do_not_create_capacity() {
    let verdict = claimability(&publication(Some(json!({"cpu": "one"})), json!({})));
    assert!(!verdict.eligible());
}
