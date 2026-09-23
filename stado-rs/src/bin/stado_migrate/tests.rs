//! Unit tests for the stado-migrate planning module. Fixtures are inline
//! registry documents; no storage, network, or credentials are touched.

use serde_json::{json, Value};

use crate::plan::build;

fn registry(coordinators: Value) -> Value {
    json!({ "coordinators": coordinators })
}

fn daemon(name: &str, active: bool, host: Option<&str>) -> Value {
    match host {
        Some(h) => json!({
            "name": name,
            "runtime": "daemon",
            "active": active,
            "host": h
        }),
        None => json!({
            "name": name,
            "runtime": "daemon",
            "active": active
        }),
    }
}

const LOCAL: &str = "local";
const SHARED: &str = "gcs";

#[test]
fn happy_path_builds_ordered_plan() {
    let doc = registry(json!([
        daemon("old-plane", true, None),
        daemon("new-plane", false, Some("operator@example-host"))
    ]));
    let plan = build(&doc, None, "new-plane", SHARED, false).expect("plan");
    assert_eq!(plan.from_name, "old-plane");
    assert_eq!(plan.to_name, "new-plane");
    assert_eq!(plan.to_host, "operator@example-host");
    assert!(!plan.move_local_storage);
    assert!(plan.steps.iter().any(|step| step.contains("flip active")));
    assert!(plan
        .steps
        .iter()
        .any(|step| step.contains("bootstrap coordinator 'new-plane'")));
}

#[test]
fn explicit_from_is_honoured() {
    let doc = registry(json!([
        daemon("old-plane", true, None),
        daemon("new-plane", false, Some("operator@example-host"))
    ]));
    let plan = build(&doc, Some("old-plane"), "new-plane", SHARED, false).expect("plan");
    assert_eq!(plan.from_name, "old-plane");
}

#[test]
fn unknown_from_is_refused() {
    let doc = registry(json!([daemon(
        "new-plane",
        false,
        Some("operator@example-host")
    )]));
    let err = build(&doc, Some("ghost"), "new-plane", SHARED, false).unwrap_err();
    assert!(err.contains("not found"), "unexpected error: {err}");
}

#[test]
fn missing_active_entry_is_refused_without_from() {
    let doc = registry(json!([
        daemon("standby", false, None),
        daemon("new-plane", false, Some("operator@example-host"))
    ]));
    let err = build(&doc, None, "new-plane", SHARED, false).unwrap_err();
    assert!(
        err.contains("no active coordinator"),
        "unexpected error: {err}"
    );
}

#[test]
fn duplicated_active_entries_are_refused() {
    let doc = registry(json!([
        daemon("one", true, None),
        daemon("two", true, None),
        daemon("new-plane", false, Some("operator@example-host"))
    ]));
    let err = build(&doc, None, "new-plane", SHARED, false).unwrap_err();
    assert!(
        err.contains("multiple active coordinators"),
        "unexpected error: {err}"
    );
}

#[test]
fn migrating_to_self_is_refused() {
    let doc = registry(json!([daemon("only", true, Some("operator@example-host"))]));
    let err = build(&doc, None, "only", SHARED, false).unwrap_err();
    assert!(
        err.contains("nothing to migrate"),
        "unexpected error: {err}"
    );
}

#[test]
fn already_active_target_is_refused() {
    let doc = registry(json!([
        daemon("old-plane", true, None),
        daemon("busy", true, Some("operator@example-host"))
    ]));
    let err = build(&doc, Some("old-plane"), "busy", SHARED, false).unwrap_err();
    assert!(
        err.contains("already the active entry"),
        "unexpected error: {err}"
    );
}

#[test]
fn non_daemon_runtime_is_refused() {
    let doc = registry(json!([
        daemon("old-plane", true, None),
        {
            "name": "cloudy",
            "runtime": "gcp_cloud_function",
            "active": false,
            "host": "operator@example-host"
        }
    ]));
    let err = build(&doc, None, "cloudy", SHARED, false).unwrap_err();
    assert!(err.contains("runtime="), "unexpected error: {err}");
}

#[test]
fn missing_target_host_is_refused() {
    let doc = registry(json!([
        daemon("old-plane", true, None),
        daemon("new-plane", false, None)
    ]));
    let err = build(&doc, None, "new-plane", SHARED, false).unwrap_err();
    assert!(err.contains("no host"), "unexpected error: {err}");
}

#[test]
fn device_local_backend_requires_the_move_flag() {
    let doc = registry(json!([
        daemon("old-plane", true, None),
        daemon("new-plane", false, Some("operator@example-host"))
    ]));
    let err = build(&doc, None, "new-plane", LOCAL, false).unwrap_err();
    assert!(err.contains("device-local"), "unexpected error: {err}");
    let plan = build(&doc, None, "new-plane", LOCAL, true).expect("plan");
    assert!(plan.move_local_storage);
    assert!(plan
        .steps
        .iter()
        .any(|step| step.contains("copy the device-local queue store")));
}

#[test]
fn document_without_coordinators_is_refused() {
    let err = build(&json!({}), None, "new-plane", SHARED, false).unwrap_err();
    assert!(
        err.contains("no coordinators array"),
        "unexpected error: {err}"
    );
}
