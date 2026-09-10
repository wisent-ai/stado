//! What arming a host from the catalog refuses, and what the registry looks
//! like afterwards.
//!
//! Each refusal is a separate sentence in the product because each sends the
//! operator to a different fix: a policy written for another platform, a
//! policy that would end somebody's session without authorization, an
//! authorization with no policy to authorize, a call that both applies a
//! policy and edits fields, and a name the catalog does not declare. A
//! refusal that left half a document behind would be worse than the failure
//! it reported, so every test also reads the registry back.

use serde_json::Value;

use crate::harness::{
    catalog, registry_bytes, setup, stado, stderr, stored_policy, INERT_POLICY, LINUX_POLICY,
    LINUX_TARGET, MACOS_POLICY, MACOS_TARGET, TARGET,
};

#[test]
fn a_policy_written_for_another_platform_is_refused_with_the_ones_that_fit() {
    let storage = setup();
    let refused = stado(
        storage.path(),
        &["space", "watermark", LINUX_TARGET, "--policy", MACOS_POLICY],
    );
    assert!(!refused.status.success());
    let said = stderr(&refused);
    assert!(
        said.contains("is written for platforms [darwin-arm64]"),
        "the refusal must name the platforms the policy is written for: {said}"
    );
    assert!(
        said.contains(LINUX_POLICY),
        "the refusal must name the policies that do fit the host: {said}"
    );
    assert_eq!(
        stored_policy(storage.path(), LINUX_TARGET),
        Value::Null,
        "a refused policy must leave the registry untouched"
    );
}

#[test]
fn a_policy_that_ends_a_graphical_session_is_refused_without_authorization() {
    let storage = setup();
    let declared = catalog(storage.path());
    let ends = crate::harness::policy_named(&declared, MACOS_POLICY);
    let processes: Vec<String> = ends["session_processes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|process| process.as_str().unwrap().to_string())
        .collect();
    assert!(
        !processes.is_empty(),
        "the policy under test must declare processes: {ends}"
    );

    let refused = stado(
        storage.path(),
        &["space", "watermark", MACOS_TARGET, "--policy", MACOS_POLICY],
    );
    assert!(!refused.status.success());
    let said = stderr(&refused);
    for process in &processes {
        assert!(
            said.contains(process),
            "the refusal must name every process it would end; {process} is missing: {said}"
        );
    }
    assert!(
        said.contains("--authorize-graphical-session"),
        "the refusal must name the flag that authorizes it: {said}"
    );
    assert_eq!(
        stored_policy(storage.path(), MACOS_TARGET),
        Value::Null,
        "a refused policy must leave the registry untouched"
    );
}

#[test]
fn authorizing_a_session_without_naming_a_policy_is_refused() {
    let storage = setup();
    let refused = stado(
        storage.path(),
        &[
            "space",
            "watermark",
            MACOS_TARGET,
            "--authorize-graphical-session",
        ],
    );
    assert_eq!(
        refused.status.code(),
        Some(2),
        "an invocation error exits 2: {}",
        stderr(&refused)
    );
    assert_eq!(
        stored_policy(storage.path(), MACOS_TARGET),
        Value::Null,
        "nothing is written by an invocation the command refused"
    );
}

#[test]
fn a_declared_policy_and_field_flags_in_one_call_are_refused() {
    let storage = setup();
    let refused = stado(
        storage.path(),
        &[
            "space",
            "watermark",
            LINUX_TARGET,
            "--policy",
            LINUX_POLICY,
            "--memory-mode",
            "off",
        ],
    );
    assert_eq!(
        refused.status.code(),
        Some(2),
        "an invocation error exits 2: {}",
        stderr(&refused)
    );
    assert_eq!(
        stored_policy(storage.path(), LINUX_TARGET),
        Value::Null,
        "a refused call must write neither the policy nor the flag"
    );
}

#[test]
fn an_unknown_policy_name_is_refused_with_every_declared_name() {
    let storage = setup();
    let declared = catalog(storage.path());
    let refused = stado(
        storage.path(),
        &[
            "space",
            "watermark",
            LINUX_TARGET,
            "--policy",
            "no-such-policy",
        ],
    );
    assert!(!refused.status.success());
    let said = stderr(&refused);
    for policy in &declared {
        let name = policy["name"].as_str().unwrap();
        assert!(
            said.contains(name),
            "the refusal must list {name}, which the catalog declares: {said}"
        );
    }
}

#[test]
fn a_host_with_no_declared_role_matches_no_policy() {
    let storage = setup();
    let mut document: Value = serde_json::from_str(&registry_bytes(storage.path())).unwrap();
    for target in document["targets"].as_array_mut().unwrap() {
        if target["name"] == TARGET {
            target.as_object_mut().unwrap().remove("role");
        }
    }
    std::fs::write(
        storage.path().join("registry.json"),
        serde_json::to_string_pretty(&document).unwrap(),
    )
    .unwrap();

    let refused = stado(
        storage.path(),
        &["space", "watermark", TARGET, "--policy", INERT_POLICY],
    );
    assert!(!refused.status.success());
    assert!(
        stderr(&refused).contains("no declared role"),
        "the refusal must say the host declares no role: {}",
        stderr(&refused)
    );
    assert_eq!(stored_policy(storage.path(), TARGET), Value::Null);
}
