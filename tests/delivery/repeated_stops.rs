//! A Stado build that stops on every start is recorded as one Oko defect,
//! rolls itself back, and a stop that cannot be reported says why.
//!
//! One isolated deployment per case, as in `broken_resolver.rs`: a local
//! store holding the shared directory template with this machine as
//! `marker-host`, and this test holding the resolution API's bind, so the real
//! resolver of `stado serve --resolver` stops the service. The real `oko`
//! keeps its defects in the deployment (`OKO_DEFECT_REGISTER`), never in the
//! person's register. Expected: the first stop is kept, raises nothing and
//! records no failed release; the second stop of the same build records one
//! stado defect naming the bind, marks that stop reported, records this
//! release as failed on the host with the earlier stop as its cause and
//! tries to reinstall the release before it, which the deployment (no
//! `stado.previous`) refuses naming that file; the third keeps the stop and
//! records nothing more. Without `oko` on the service's path, the second
//! stop is kept with the cause it could not be reported.
mod stopping_deployment;

use serde_json::Value;

use stopping_deployment::{Deployment, TARGET};

#[test]
fn a_build_that_stops_again_is_one_oko_defect() {
    let deployment = Deployment::start(std::env::var_os("PATH").expect("PATH names oko"));
    let (holder, api_bind) = deployment.broken();

    deployment.stop();
    let first = deployment.stops();
    assert_eq!(
        first.len(),
        first
            .iter()
            .filter(|stop| stop["reported"] == false)
            .count()
    );
    assert!(deployment.defects().is_empty(), "one stop raised a defect");
    assert!(
        deployment.failed_record().is_none(),
        "one stop recorded the release as failed"
    );

    let said = deployment.stop();
    let stops = deployment.stops();
    let latest = stops.last().unwrap();
    let report = latest["report"].as_str().unwrap();
    assert!(
        latest["reported"] == true,
        "the second stop is not reported: {said}"
    );
    assert!(
        report.starts_with("recorded:"),
        "oko did not record it: {report}"
    );
    let defects = deployment.defects();
    assert_eq!(defects.len(), stops.len() - first.len(), "{defects:#?}");
    let defect = defects.first().unwrap();
    assert!(
        defect["title"].as_str().unwrap().contains(&api_bind),
        "the defect does not name the resolver's cause: {defect:#?}"
    );
    assert!(
        defect["evidence"]
            .as_str()
            .unwrap()
            .contains(env!("CARGO_PKG_VERSION")),
        "{defect:#?}"
    );
    // The second stop proves the install failed: the release records itself
    // failed with the earlier stop as the cause and tries the release before
    // it, which this deployment does not retain.
    let Some(record) = deployment.failed_record() else {
        panic!("the second stop recorded no failed release under the deployment's retained releases: {said}");
    };
    let cause = record["cause"].as_str().unwrap();
    assert!(
        cause.contains("stopped 1 time(s) on this host before this one")
            && cause.contains(&api_bind),
        "the record does not name the earlier stop and the bind: {record:#}"
    );
    assert!(
        said.contains(
            "this release failed on this host and the release before it could not be reinstalled:"
        ) && said.contains("stado.previous"),
        "the refused rollback is not said with the missing target: {said}"
    );

    deployment.stop();
    assert_eq!(
        deployment.defects().len(),
        defects.len(),
        "a third stop raised another defect"
    );
    let last = deployment.stops();
    assert!(last.last().unwrap()["reported"] == false, "{last:#?}");

    // `stado resolver status` names the stopped build and its defect.
    let output = deployment
        .command()
        .args(["resolver", "status", "--target", TARGET, "--json"])
        .output()
        .unwrap();
    let status: Value = match serde_json::from_slice(&output.stdout) {
        Ok(status) => status,
        Err(error) => panic!(
            "resolver status printed no report ({error}): {}",
            String::from_utf8_lossy(&output.stderr)
        ),
    };
    let stopped = &status["serve_stops"];
    assert_eq!(
        stopped["stops"].as_u64(),
        Some(last.len() as u64),
        "{status:#}"
    );
    assert_eq!(stopped["version"], env!("CARGO_PKG_VERSION"), "{status:#}");
    assert!(
        stopped["report"]
            .as_str()
            .is_some_and(|report| report.starts_with("recorded:")),
        "{status:#}"
    );
    // The reported stop holds its revision from source installs.
    assert_eq!(stopped["held"], true, "{status:#}");
    drop(holder);
}

#[test]
fn a_stop_that_cannot_reach_oko_keeps_its_cause() {
    // The tools stado serve needs, without the directory that holds oko.
    let deployment = Deployment::start(std::ffi::OsString::from("/usr/bin:/bin:/usr/sbin:/sbin"));
    let (holder, _) = deployment.broken();
    deployment.stop();
    let said = deployment.stop();
    let stops = deployment.stops();
    let report = stops.last().unwrap()["report"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        report.starts_with("oko defects record could not run:"),
        "the cause is not kept: {report}"
    );
    assert!(said.contains(&report), "the cause is not said: {said}");
    assert!(
        stops.last().unwrap()["reported"] == false,
        "a report Oko never took is not reported: {stops:#?}"
    );

    // The next stop tries again, and says why it still could not.
    let again = deployment.stop();
    let retried = deployment.stops();
    let latest = retried.last().unwrap()["report"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        latest.starts_with("oko defects record could not run:") && again.contains(&latest),
        "the next stop did not try to report again: {retried:#?}"
    );
    drop(holder);
}
