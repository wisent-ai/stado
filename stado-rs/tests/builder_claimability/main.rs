//! A release builder must be able to claim work, not merely publish a heartbeat.
//!
//! Builder selection consumes the worker's explicit live admission decision.
//! The old inferred slot table is deliberately ignored: during a rolling
//! upgrade a publication without `accepting_jobs` is unknown, while an explicit
//! false is authoritative and carries the same blockers as `stado host gates`.

use serde_json::{json, Value};

use stado::cli::release_submit::{claimability, Claimability};
use stado::deploy::host_gates::{
    DISK_CLEANUP_POLICY_UNKNOWN, DISK_CLEANUP_STALLED, DISK_PRESSURE_ACTIVE,
    DISK_PRESSURE_UNRESOLVED, QUEUE_PAUSED,
};

fn publication(accepting_jobs: Option<Value>, available_cpu_cores: Option<i64>, diag: Value) -> Value {
    let mut payload = json!({
        "consumer_id": "local-charless-mac-mini.local",
        "kind": "local",
        "running_jobs": 0,
        "total_cpu_cores": 12,
        "free_ram_gb": 32,
        "total_ram_gb": 64,
        "free_vram_gb": 0,
        "total_vram_gb": 0,
        "available_accelerators": {},
        "published_at": "2026-09-04T17:42:17.379737+00:00",
        "diag": diag,
    });
    if let Some(accepting) = accepting_jobs {
        payload["accepting_jobs"] = accepting;
    }
    if let Some(cores) = available_cpu_cores {
        payload["available_cpu_cores"] = json!(cores);
    }
    payload
}

#[test]
fn an_accepting_worker_is_claimable_with_its_live_cpu_count() {
    let verdict = claimability(&publication(Some(json!(true)), Some(7), json!({})));
    assert_eq!(
        verdict,
        Claimability::Claimable {
            available_cpu_cores: 7
        }
    );
    assert!(verdict.eligible());
    assert!(verdict.describe().contains("7 CPU core(s) available"));
}

#[test]
fn an_explicit_refusal_is_never_selected_even_if_old_slots_say_free() {
    let mut payload = publication(
        Some(json!(false)),
        Some(8),
        json!({"admission_reason": "ram_exhausted"}),
    );
    payload["free_slots"] = json!({"cpu": 99});
    let verdict = claimability(&payload);
    assert_eq!(
        verdict,
        Claimability::Refusing {
            blockers: vec!["ram_exhausted".to_string()]
        }
    );
    assert!(!verdict.eligible());
    assert!(verdict.describe().contains("ram_exhausted"));
}

#[test]
fn a_refusal_carries_the_host_gate_reasons_the_worker_published() {
    let verdict = claimability(&publication(
        Some(json!(false)),
        Some(0),
        json!({
            "disk_pressure_active": true,
            "disk_pressure_unresolved": true,
            "disk_cleanup_policy_known": false,
            "queue_paused": true,
            "disk_cleanup": {"outcome": "lock_busy", "lock_busy": true},
        }),
    ));
    let Claimability::Refusing { blockers } = &verdict else {
        panic!("expected a refusal, got {verdict:?}");
    };
    assert!(blockers.iter().any(|b| b.starts_with(DISK_PRESSURE_ACTIVE)));
    assert!(blockers.iter().any(|b| b == DISK_PRESSURE_UNRESOLVED));
    assert!(blockers.iter().any(|b| b == DISK_CLEANUP_POLICY_UNKNOWN));
    assert!(blockers.iter().any(|b| b == QUEUE_PAUSED));
    assert!(blockers.iter().any(|b| b.starts_with(DISK_CLEANUP_STALLED)));
}

#[test]
fn an_explicit_refusal_without_a_reason_stays_a_refusal() {
    let verdict = claimability(&publication(Some(json!(false)), Some(0), json!({})));
    assert_eq!(
        verdict,
        Claimability::Refusing {
            blockers: Vec::new()
        }
    );
    assert!(!verdict.eligible());
    assert_eq!(verdict.describe(), "not accepting jobs; no reason published");
}

#[test]
fn a_rolling_upgrade_publication_without_a_decision_is_unstated() {
    for legacy_shape in [
        json!({"cpu": 0}),
        json!({"cpu": 4}),
        json!({}),
        Value::Null,
    ] {
        let mut payload = publication(None, None, json!({"storage_backend": "stado"}));
        payload["free_slots"] = legacy_shape;
        let verdict = claimability(&payload);
        assert_eq!(verdict, Claimability::Unstated);
        assert!(
            verdict.eligible(),
            "missing new admission data is unknown during a rolling upgrade, not a refusal"
        );
        assert_eq!(verdict.describe(), "published no admission decision");
    }
}

#[test]
fn a_non_boolean_admission_value_is_unstated_not_invented() {
    for malformed in [json!(1), json!("yes"), json!([]), json!({})] {
        let verdict = claimability(&publication(Some(malformed.clone()), Some(5), json!({})));
        assert_eq!(verdict, Claimability::Unstated, "shape {malformed:?}");
    }
}
