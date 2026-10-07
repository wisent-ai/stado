//! Real `stado build status` reads of an operator-declared build whose
//! platform job ended without a receipt (cancelled, or failed before its
//! worker wrote one) and whose queue record the run reaper has since retired.
//! The build, its job and the run manifest come from the normal Stado
//! lifecycle against the declared qualification configuration; nothing is
//! seeded.
#[path = "../desktop_api/fixture.rs"]
mod fixture;
use fixture::Service;
use serde_json::{json, Value};
use std::path::Path;

/// One command line as the operator types it, split into its arguments.
fn argv(line: &str) -> Vec<&str> {
    line.split_whitespace().collect()
}

/// The platforms whose job the build still calls submitted.
fn building(build: &Value) -> Vec<String> {
    build["platforms"]
        .as_object()
        .expect("a build lists its platforms")
        .iter()
        .filter(|(_, platform)| platform["state"] == "submitted")
        .map(|(name, _)| name.clone())
        .collect()
}

/// The platforms whose failure names the job's own terminal state.
fn ended<'a>(build: &'a Value, state: &str) -> Vec<&'a Value> {
    build["platforms"]
        .as_object()
        .expect("a build lists its platforms")
        .values()
        .filter(|platform| {
            platform["failure"]
                .as_str()
                .is_some_and(|failure| failure.contains(&format!("ended {state}")))
        })
        .collect()
}

#[tokio::test]
async fn a_job_the_reaper_retired_without_a_receipt_reads_as_failed() {
    let mut service = Service::start_with_configuration("STADO_BUILD_QUALIFICATION_CONFIG");
    let id = service.input("STADO_BUILD_REAPED_CANCELLED_ID");
    let line = format!("build status {id} --json");
    let args = argv(&line);
    let cli: Value = serde_json::from_str(&service.cli(&args)).unwrap();
    let api = service
        .call(json!({ "args": args }), reqwest::StatusCode::OK.as_u16())
        .await;
    assert_eq!(api["ok"], true, "{api}");
    let api: Value = serde_json::from_str(api["stdout"].as_str().unwrap()).unwrap();
    for build in [&cli, &api] {
        let cancelled = ended(build, "cancelled");
        assert!(
            !cancelled.is_empty(),
            "no platform reads as ended cancelled: {build}"
        );
        assert!(
            cancelled
                .iter()
                .all(|platform| platform["state"] == "failed"),
            "{build}"
        );
        assert!(
            building(build).is_empty(),
            "a platform whose job ended still reads as building: {build}"
        );
        assert_ne!(build["state"], "waiting", "{build}");
    }
    // The read saved what it found, so the next read answers from the record.
    let again: Value = serde_json::from_str(&service.cli(&args)).unwrap();
    assert_eq!(again["platforms"], cli["platforms"]);
    service.observe("build", cli);
    service.pass();
}

#[tokio::test]
async fn an_unknown_build_is_refused_by_name() {
    let mut service = Service::start_with_configuration("STADO_BUILD_QUALIFICATION_CONFIG");
    let id = uuid::Uuid::new_v4().simple().to_string();
    let line = format!("build status {id}");
    let output = service.execute(Path::new(env!("CARGO_BIN_EXE_stado")), &argv(&line));
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&format!("build {id} does not exist")),
        "the refusal must name the build: {stderr}"
    );
    service.pass();
}
