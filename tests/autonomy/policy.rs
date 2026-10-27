//! A stored autonomy policy Stado cannot read is refused by name through the
//! real CLI.
//!
//! One isolated deployment (its own HOME and local store): a policy object
//! that is not JSON, and then one whose field has the wrong type, is written
//! where the operator's policy lives (`state/autonomy/policy.json`), and the
//! real `stado optimize status` reads it. Each must exit unsuccessfully,
//! naming the policy object and what serde could not read in it, and print
//! no status as if autonomy ran without limits.
use std::fs;

use crate::deployment::Deployment;
use serde_json::json;

/// Where the operator's autonomy policy object lives in a store.
const POLICY_OBJECT: &str = "state/autonomy/policy.json";

/// Write `body` as the stored policy and read it through the real CLI; the
/// refusal's standard error.
fn refused(deployment: &mut Deployment, body: &str) -> String {
    let object = deployment.store().join(POLICY_OBJECT);
    fs::create_dir_all(object.parent().expect("the policy object has a folder")).unwrap();
    fs::write(&object, body).unwrap();
    let (succeeded, stdout, stderr) = deployment.run(&["optimize", "status", "--json"], "");
    assert!(
        !succeeded,
        "stado optimize status read the unreadable policy {body:?} as a status: {stdout}"
    );
    assert!(
        stderr.contains(POLICY_OBJECT),
        "the refusal names the policy object: {stderr}"
    );
    assert!(
        stderr.contains("is not an autonomy policy this Stado reads"),
        "the refusal says the object is not a policy this Stado reads: {stderr}"
    );
    stderr
}

#[test]
fn an_unreadable_autonomy_policy_is_refused_naming_its_object_and_reason() {
    let mut deployment = Deployment::start("autonomy-policy");
    let local = deployment.store();
    deployment.set("WC_STORAGE_BACKEND", "local");
    deployment.set(
        "WC_LOCAL_STORAGE_PATH",
        local.to_str().expect("the run directory is UTF-8"),
    );

    let malformed = refused(&mut deployment, "{ this is not JSON");
    deployment.report["malformed"] = json!(malformed);

    let invalid = refused(&mut deployment, r#"{"policy_version": ["not", "text"]}"#);
    assert!(
        invalid.contains("invalid type"),
        "the refusal states serde's reason: {invalid}"
    );
    deployment.report["invalid"] = json!(invalid);
    deployment.pass();
}
