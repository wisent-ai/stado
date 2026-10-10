//! `stado release status <product>` of a real release run whose platform job
//! failed prints the failure as the run recorded it, whole: the job's last
//! output lines travel in the failure field, and the line register shows
//! them under the platform instead of ending at "the job's last output:".
//! The runs come from the normal Stado lifecycle in the declared real queue;
//! nothing is seeded. Every recorded run is read as JSON, each platform
//! whose failure carries lines after its first is the case, and the line
//! register of that product must show every one of those lines.
//!
//! Run with `cargo test --manifest-path tests/status/Cargo.toml --test
//! build-status-failure-lines -- --ignored --nocapture` with
//! `STADO_RELEASE_QUALIFICATION_CONFIG` naming the declared queue's
//! configuration file; without that declaration the journey is ignored,
//! never passed.
#[path = "../status/fixture.rs"]
mod fixture;
use fixture::Store;
use serde_json::{json, Value};

/// The declared configuration file of the real queue.
const CONFIGURATION: &str = "STADO_RELEASE_QUALIFICATION_CONFIG";

/// Whether `failure` carries lines after its first: the job output the
/// worker wrote, which the register must show.
fn spans_lines(failure: &str) -> bool {
    let mut lines = failure.lines();
    lines.next();
    lines.next().is_some()
}

#[test]
#[ignore = "requires STADO_RELEASE_QUALIFICATION_CONFIG naming the declared real queue's configuration"]
fn a_failed_platform_shows_every_line_of_its_failure() {
    let mut store = Store::declared("release-status-failure-lines", CONFIGURATION);
    let printed = store.ok(&["release", "status", "--json"]);
    let listing: Value = serde_json::from_str(&printed).expect("--json prints JSON");
    let runs = listing["runs"]
        .as_array()
        .expect("the listing names its runs")
        .clone();
    let mut cases = Vec::new();
    for run in &runs {
        let Some(product) = run["product"].as_str() else {
            continue;
        };
        let Some(platforms) = run["platforms"].as_object() else {
            continue;
        };
        for (name, platform) in platforms {
            let Some(failure) = platform["failure"].as_str() else {
                continue;
            };
            if spans_lines(failure) {
                cases.push((
                    product.to_owned(),
                    run["run_id"].clone(),
                    name.clone(),
                    failure.to_owned(),
                ));
            }
        }
    }
    assert!(
        !cases.is_empty(),
        "blocked: the declared queue records no run whose platform failure carries lines after its first"
    );
    for (product, run_id, name, failure) in &cases {
        let register = store.ok(&["release", "status", product]);
        for line in failure.lines() {
            assert!(
                register.contains(line.trim()),
                "{run_id} {name}: the line register of {product} does not show this failure line: {line}"
            );
        }
    }
    store.observe("cases", json!(cases));
    store.pass();
}
