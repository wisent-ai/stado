//! The `weles-browser-task` outcome is the run document the Weles API
//! answers: the trajectory's own `result`, the named documents the run left
//! under `outputs` (`pangram`, `overleaf_version_history_summary`, …), the
//! files it could not read under `output_errors`, and the streams. A
//! document missing a declared field is refused naming the field, not read
//! as a run that left nothing; `--json` prints `outputs` beside `result`.
use serde_json::{json, Value};
use stado::deploy::weles_browser_task::outcome;

const ACTION: &str = "overleaf_version_history_scan";

/// A run document as Weles's trajectory runner writes it: the trajectory
/// printed no JSON line, so `result` is null, and the summary it wrote is
/// under `outputs`.
fn run_document() -> Value {
    json!({
        "ok": true,
        "exitCode": null,
        "action": ACTION,
        "run_id": "run-probe",
        "result": null,
        "outputs": {
            "overleaf_version_history_summary": {"projectId": "0123456789abcdef01234567", "probes": []}
        },
        "output_errors": {"pangram_result.json": "Unexpected end of JSON input"},
        "stdout_tail": "[version_history_ui] done",
        "stderr_tail": "",
        "cancelled": false
    })
}

#[test]
fn the_report_carries_the_outputs_and_the_unreadable_ones_by_name() {
    let outcome = outcome(&run_document(), ACTION, None).expect("a complete run document reads");
    assert!(outcome.ok);
    assert_eq!(
        outcome.exit_code, None,
        "a null exit code is a run a signal ended"
    );
    let report = outcome.to_report("marker-host", ACTION);
    assert_eq!(report["host"], json!("marker-host"));
    assert_eq!(report["run_id"], json!("run-probe"));
    assert_eq!(
        report["outputs"]["overleaf_version_history_summary"]["projectId"],
        json!("0123456789abcdef01234567")
    );
    assert_eq!(
        report["output_errors"]["pangram_result.json"],
        json!("Unexpected end of JSON input")
    );
    assert!(report["result"].is_null());
}

#[test]
fn a_run_document_without_outputs_is_refused_naming_the_field() {
    let mut document = run_document();
    document.as_object_mut().unwrap().remove("outputs");
    let refusal = outcome(&document, ACTION, None).err().expect("refused");
    let said = refusal.to_string();
    assert!(said.contains("carries no `outputs`"), "{said}");
    assert!(said.contains(ACTION), "{said}");
}

#[test]
fn a_run_document_whose_ok_is_not_a_boolean_is_refused_naming_the_field() {
    let mut document = run_document();
    document["ok"] = json!("yes");
    let refusal = outcome(&document, ACTION, None).err().expect("refused");
    assert!(
        refusal.to_string().contains("`ok` is not a boolean"),
        "{refusal}"
    );
}
