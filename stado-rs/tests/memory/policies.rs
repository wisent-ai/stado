//! Arming a host from the declared policy catalog, through the real binary
//! and the real registry writer.
//!
//! Applying a declared policy is registry work — the fit is read from
//! `release_platform` and `role` — so it is proved on either platform against
//! the two platform fixtures, and nothing is executed against them. The only
//! policy this suite ever applies to the machine it runs on is
//! `observe-only`, which repairs nothing.

use serde_json::Value;

use crate::harness::{
    catalog, declare, policy_named, run_pass, setup, stado, stderr, stdout, stored_policy,
    INERT_POLICY, LINUX_POLICY, LINUX_TARGET, MACOS_POLICY, MACOS_TARGET, TARGET,
};

#[test]
fn a_declared_policy_is_written_whole_and_read_back_as_armed() {
    let storage = setup();
    let declared = catalog(storage.path());
    let expected = policy_named(&declared, LINUX_POLICY)["policy"].clone();

    let applied = stado(
        storage.path(),
        &[
            "space",
            "watermark",
            LINUX_TARGET,
            "--policy",
            LINUX_POLICY,
            "--json",
        ],
    );
    assert!(
        applied.status.success(),
        "applying {LINUX_POLICY} failed: {}",
        stderr(&applied)
    );
    let receipt: Value = serde_json::from_str(&stdout(&applied)).unwrap();
    assert_eq!(receipt["applied_policy"], Value::from(LINUX_POLICY));
    assert_eq!(receipt["memory_reclaim"], expected);
    assert_eq!(
        stored_policy(storage.path(), LINUX_TARGET),
        expected,
        "the registry must carry the declared policy unchanged"
    );

    let read = stado(
        storage.path(),
        &["space", "watermark", LINUX_TARGET, "--json"],
    );
    let verdict: Value = serde_json::from_str(&stdout(&read)).unwrap();
    assert_eq!(verdict["automatic"]["armed"], Value::Bool(true));
    assert_eq!(
        verdict["automatic"]["reviewed_policy"],
        Value::from(LINUX_POLICY)
    );
}

#[test]
fn authorizing_a_session_policy_writes_the_authorization_it_declares() {
    let storage = setup();
    let declared = catalog(storage.path());
    let ends = policy_named(&declared, MACOS_POLICY);
    assert_eq!(
        ends["ends_graphical_session"],
        Value::Bool(true),
        "this test is about the policy that ends session processes: {ends}"
    );

    let authorized = stado(
        storage.path(),
        &[
            "space",
            "watermark",
            MACOS_TARGET,
            "--policy",
            MACOS_POLICY,
            "--authorize-graphical-session",
        ],
    );
    assert!(
        authorized.status.success(),
        "the authorized write must be accepted: {}",
        stderr(&authorized)
    );
    let stored = stored_policy(storage.path(), MACOS_TARGET);
    assert_eq!(
        stored["repairs"]["graphical_session"]["allow_graphical_session"],
        Value::Bool(true)
    );
    assert_eq!(
        stored["repairs"]["graphical_session"]["processes"], ends["session_processes"],
        "the host must permit exactly the processes the catalog declares"
    );
    assert_eq!(
        stored["repairs"]["restart_unit"]["allow_graphical_session"],
        Value::Null,
        "a repair that cannot end a session must not carry the authorization"
    );
}

#[test]
fn a_hand_written_declaration_is_reported_as_reviewed_by_nobody() {
    let storage = setup();
    let written = declare(storage.path(), &["--memory-mode", "report"]);
    assert!(written.status.success(), "{}", stderr(&written));

    let read = stado(storage.path(), &["space", "watermark", TARGET, "--json"]);
    let verdict: Value = serde_json::from_str(&stdout(&read)).unwrap();
    assert_eq!(verdict["automatic"]["armed"], Value::Bool(false));
    assert_eq!(
        verdict["automatic"]["reviewed_policy"],
        Value::Null,
        "a declaration outside the catalog is reviewed by nobody: {verdict}"
    );
    let printed = stdout(&stado(storage.path(), &["space", "watermark", TARGET]));
    assert!(
        printed.contains("written by hand"),
        "the operator must be told the document is hand-written: {printed}"
    );
}

#[test]
fn an_inert_declared_policy_is_executed_by_a_real_pass() {
    let storage = setup();
    let applied = stado(
        storage.path(),
        &["space", "watermark", TARGET, "--policy", INERT_POLICY],
    );
    assert!(
        applied.status.success(),
        "applying {INERT_POLICY} failed: {}",
        stderr(&applied)
    );

    let report = run_pass(storage.path());
    assert_eq!(report["mode"], Value::from("report"));
    let repairs = report["repairs"].as_object().cloned().unwrap_or_default();
    assert!(
        repairs.is_empty(),
        "a policy that declares no repair must perform none: {report}"
    );
    assert!(
        report["memory_before"]["available_bytes"]
            .as_i64()
            .is_some_and(|bytes| bytes > 0),
        "the pass must record what it read: {report}"
    );
    assert!(
        report["policy_digest"]
            .as_str()
            .is_some_and(|digest| !digest.is_empty()),
        "the pass must record which policy document it resolved: {report}"
    );
    assert_eq!(
        report["policy_defaulted"],
        Value::Bool(false),
        "a declared policy must not be reported as the reporting default: {report}"
    );

    let read = stado(storage.path(), &["space", "policies", TARGET, "--json"]);
    let verdict: Value = serde_json::from_str(&stdout(&read)).unwrap();
    assert_eq!(verdict["automatic"]["armed"], Value::Bool(false));
    assert_eq!(
        verdict["automatic"]["reviewed_policy"],
        Value::from(INERT_POLICY),
        "a host carrying a declared policy must be reported as carrying it: {verdict}"
    );
}
