//! `stado release status --run <id>` of an operator-declared real release
//! run whose platform job ended without a receipt (cancelled, or failed
//! before its worker wrote one) and whose queue record the run reaper has
//! since retired. The run, its job and the retained outcome come from the
//! normal Stado lifecycle against the declared qualification configuration;
//! nothing is seeded. The platform reads as failed with the job's own end
//! (`ended cancelled`), the run as failed, and the leg is not reported lost.
#[path = "../status/fixture.rs"]
mod fixture;
use fixture::Store;
use serde_json::{json, Value};

/// The declared configuration file of the real queue, and the run to read.
const CONFIGURATION: &str = "STADO_RELEASE_QUALIFICATION_CONFIG";
const RUN: &str = "STADO_RELEASE_REAPED_RUN_ID";

#[test]
fn a_platform_whose_reaped_job_left_no_receipt_reads_as_ended_cancelled() {
    let mut store = Store::declared("release-status-reaped-platform", CONFIGURATION);
    let run_id = match std::env::var(RUN) {
        Ok(value) if !value.trim().is_empty() => value,
        _ => panic!("a real declared-run journey requires {RUN}"),
    };
    let printed = store.ok(&["release", "status", "--run", &run_id, "--json"]);
    let listing: Value = serde_json::from_str(&printed).expect("--json prints JSON");
    let runs = listing["runs"].as_array().expect("the listing names its runs");
    let run = runs
        .iter()
        .find(|run| run["run_id"].as_str().is_some_and(|id| id.starts_with(&run_id)))
        .expect("the declared run is listed");
    let platforms = run["platforms"].as_object().expect("a run lists its platforms");
    let ended: Vec<(&String, &Value)> = platforms
        .iter()
        .filter(|(_, platform)| {
            platform["failure"]
                .as_str()
                .is_some_and(|failure| failure.contains("ended cancelled"))
        })
        .collect();
    assert!(
        !ended.is_empty(),
        "no platform reads as ended cancelled: {run}"
    );
    for (name, platform) in &ended {
        assert_eq!(platform["state"], "failed", "{name}: {platform}");
        assert_eq!(platform["job_state"], "cancelled", "{name}: {platform}");
        assert!(
            !platform["failure"]
                .as_str()
                .is_some_and(|failure| failure.contains("the job was lost")),
            "{name} is reported lost instead of ended: {platform}"
        );
    }
    assert_eq!(run["phase"], "failed", "{run}");
    store.observe("run", json!(run));
    store.pass();
}
