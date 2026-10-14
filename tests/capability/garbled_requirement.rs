//! A published job requirement whose trajectory value is not a list of
//! capability ids is refused by name, not read as a requirement every host
//! meets.
//!
//! One isolated deployment: `config init` seeds a local registry naming this
//! machine; the test declares on that target a service running a trajectory,
//! publishes `job_requirements/<name>.json` naming the trajectory with a value
//! that is not a list, and reads the real `stado registry doctor --json`.
// This journey reads the deployment's store and CLI only; the beacon serving
// the host-health journeys drive through it is unused here.
#[path = "../host_health/beacon/deployment.rs"]
#[allow(dead_code)]
mod deployment;

use deployment::Deployment;
use serde_json::{json, Value};
use std::fs;

const SCHEMA: &str = "wisent.trajectory-requirements.v1";
const TRAJECTORY: &str = "garbled-requirement/login";
const SERVICE: &str = "garbled-requirement-unit";

/// Declare on the seeded target one service that runs `TRAJECTORY`.
fn declare_service(deployment: &Deployment) {
    let path = deployment.store().join("registry.json");
    let mut registry: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let target = registry["targets"]
        .as_array_mut()
        .and_then(|targets| targets.first_mut())
        .expect("config init seeds a registry naming this machine");
    target["services"] = json!([{"name": SERVICE, "trajectory": TRAJECTORY}]);
    fs::write(&path, serde_json::to_vec_pretty(&registry).unwrap()).unwrap();
}

/// Publish `body` as a job requirement declaration.
fn publish(deployment: &Deployment, body: &Value) {
    let directory = deployment.store().join("job_requirements");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("garbled-requirement.json"),
        serde_json::to_vec_pretty(body).unwrap(),
    )
    .unwrap();
}

/// The `capability-unsatisfied` details `stado registry doctor --json` reports.
fn capability_findings(deployment: &mut Deployment) -> Vec<String> {
    let output = deployment.run(&["registry", "doctor", "--json"]);
    let report: Value = match serde_json::from_slice(&output.stdout) {
        Ok(report) => report,
        Err(error) => panic!("registry doctor printed no JSON report: {error}"),
    };
    report["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|finding| finding["finding"] == "capability-unsatisfied")
        .filter_map(|finding| finding["detail"].as_str().map(str::to_string))
        .collect()
}

#[test]
fn a_garbled_requirement_is_refused_by_name() {
    let mut deployment = Deployment::start();
    declare_service(&deployment);

    // A trajectory whose value is not a list names no capability ids; it used
    // to read as an empty list, a job every host can run.
    publish(
        &deployment,
        &json!({"schema": SCHEMA, "trajectories": {TRAJECTORY: "browser-login"}}),
    );
    let findings = capability_findings(&mut deployment);
    deployment.report["garbled"] = json!(findings);
    assert!(
        findings.iter().any(|detail| detail.contains(SERVICE)
            && detail.contains(&format!("declares trajectory {TRAJECTORY} as"))
            && detail.contains("not a list of capability ids")),
        "{findings:?}"
    );

    // A requirement naming no schema says so rather than quoting an empty one.
    publish(&deployment, &json!({"trajectories": {TRAJECTORY: []}}));
    let findings = capability_findings(&mut deployment);
    deployment.report["no_schema"] = json!(findings);
    assert!(
        findings
            .iter()
            .any(|detail| detail.contains("names no schema") && detail.contains(SCHEMA)),
        "{findings:?}"
    );

    // A well-formed requirement that needs nothing is met by every host.
    publish(
        &deployment,
        &json!({"schema": SCHEMA, "trajectories": {TRAJECTORY: []}}),
    );
    let findings = capability_findings(&mut deployment);
    deployment.report["well_formed"] = json!(findings);
    assert!(
        !findings.iter().any(|detail| detail.contains(TRAJECTORY)),
        "{findings:?}"
    );
    deployment.pass();
}
