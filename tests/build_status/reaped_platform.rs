//! `stado release status --run <id> --json` of a real release run whose
//! platform job ended without a receipt (cancelled, or failed before its
//! worker wrote one) and whose queue record the run reaper has since retired.
//! The run, its job and the retained outcome come from the normal Stado
//! lifecycle in the declared real queue; nothing is seeded, and the run is
//! found in that queue by the product's own reads: every recorded run's
//! platforms are listed, and a platform whose job `stado status` answers
//! from a run's retained outcome as cancelled is the case. For each such
//! platform the release read says `failed` with the job's own end
//! (`ended cancelled`), not that the job was lost.
//!
//! Run with `cargo test --manifest-path tests/status/Cargo.toml --test
//! build-status-reaped-platform -- --ignored --nocapture` with
//! `STADO_RELEASE_QUALIFICATION_CONFIG` naming the declared queue's
//! configuration file; without that declaration the journey is ignored,
//! never passed.
#[path = "../status/fixture.rs"]
mod fixture;
use fixture::Store;
use serde_json::{json, Value};

/// The declared configuration file of the real queue.
const CONFIGURATION: &str = "STADO_RELEASE_QUALIFICATION_CONFIG";

/// Whether `stado status` answers `job_id` from a run's retained outcome as
/// a cancelled job: the queue holds no record of it any more.
fn reaped_cancelled(store: &mut Store, job_id: &str) -> bool {
    let output = store.run(&["status", job_id, "--json"]);
    if !output.status.success() {
        return false;
    }
    let rows: Value = serde_json::from_slice(&output.stdout).expect("--json prints JSON");
    rows.as_array().is_some_and(|rows| {
        rows.iter()
            .any(|row| row["reaped"] == true && row["state"] == "cancelled")
    })
}

#[test]
#[ignore = "requires STADO_RELEASE_QUALIFICATION_CONFIG naming the declared real queue's configuration"]
fn a_platform_whose_reaped_job_left_no_receipt_reads_as_ended_cancelled() {
    let mut store = Store::declared("release-status-reaped-platform", CONFIGURATION);
    let printed = store.ok(&["release", "status", "--json"]);
    let listing: Value = serde_json::from_str(&printed).expect("--json prints JSON");
    let runs = listing["runs"]
        .as_array()
        .expect("the listing names its runs")
        .clone();
    let mut cases = Vec::new();
    for run in &runs {
        let Some(platforms) = run["platforms"].as_object() else {
            continue;
        };
        for (name, platform) in platforms {
            let Some(job_id) = platform["job_id"].as_str().filter(|id| !id.is_empty()) else {
                continue;
            };
            if reaped_cancelled(&mut store, job_id) {
                cases.push((run["run_id"].clone(), name.clone(), platform.clone()));
            }
        }
    }
    assert!(
        !cases.is_empty(),
        "blocked: the declared queue records no run whose platform job was cancelled and reaped"
    );
    for (run_id, name, platform) in &cases {
        assert_eq!(platform["state"], "failed", "{run_id} {name}: {platform}");
        assert_eq!(platform["job_state"], "cancelled", "{run_id} {name}: {platform}");
        let failure = platform["failure"].as_str().expect("a failed platform names its failure");
        assert!(
            failure.contains("ended cancelled"),
            "{run_id} {name}: {failure}"
        );
        assert!(
            !failure.contains("the job was lost"),
            "{run_id} {name} is reported lost instead of ended: {failure}"
        );
    }
    store.observe("cases", json!(cases));
    store.pass();
}
