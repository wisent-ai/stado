//! `stado ci delivery` names every way the delivery path is down, so Oko can
//! order work by it.
//!
//! Runs the built `stado ci delivery --json` with a delivery state of the
//! run's own (STADO_DELIVERY_STATE, in the shape a pass writes it). First the
//! state holds a refused step with its refusal and the Oko defect recorded
//! for it: `stalled` names the step, its revision and the refusal, and
//! `blocking_defects` carries the defect. Then the state holds a step
//! recorded as installing while no pass holds the routine's lock: `stalled`
//! says the pass ended without an outcome. The unit read of this machine is
//! reported either way, as a state or as its read error, never left out.
//! Nothing of the operator's is read or written.
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};

fn delivery(state: &Path) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(["ci", "delivery", "--json"])
        .env("STADO_DELIVERY_STATE", state)
        .output()
        .expect("the built stado starts");
    assert!(
        output.status.success(),
        "stado ci delivery refused: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("the report is JSON")
}

fn stalled(report: &Value) -> Vec<String> {
    report["stalled"]
        .as_array()
        .expect("stalled is a list")
        .iter()
        .map(|reason| reason.as_str().expect("each reason is text").to_string())
        .collect()
}

#[test]
fn a_refused_step_and_an_abandoned_install_stall_the_path() {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("delivery-path-stalled-{}", std::process::id()));
    std::fs::create_dir_all(&root).expect("the run directory is created");
    let state = root.join("delivery-state.json");

    std::fs::write(
        &state,
        json!({
            "refused": {
                "step": "fixture:service", "revision": "fixture-revision",
                "refusals": ["the fixture's quality gate refused"],
                "defects": ["fixture-defect"], "at": "2026-10-14T07:00:00Z"
            }
        })
        .to_string(),
    )
    .expect("the state is written");
    let report = delivery(&state);
    let reasons = stalled(&report);
    assert!(
        reasons.iter().any(|reason| reason
            .contains("step fixture:service at fixture-revision was refused")
            && reason.contains("the fixture's quality gate refused")),
        "{report}"
    );
    assert_eq!(
        report["blocking_defects"],
        json!(["fixture-defect"]),
        "{report}"
    );
    assert!(
        report["unit"]["read_error"].is_string() || report["unit"]["loaded"].is_boolean(),
        "the unit is reported: {report}"
    );

    std::fs::write(
        &state,
        json!({
            "running": {
                "step": "fixture:service", "revision": "fixture-revision",
                "started_at": "2026-10-14T07:00:00Z"
            }
        })
        .to_string(),
    )
    .expect("the state is written");
    let report = delivery(&state);
    assert!(
        stalled(&report)
            .iter()
            .any(|reason| reason.contains("ended without recording an outcome")),
        "{report}"
    );
    assert_eq!(report["pass_lock_holder"], Value::Null, "{report}");

    std::fs::remove_dir_all(&root).expect("the run directory is removed");
}
