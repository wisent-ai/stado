//! The surfaces an operator reads, and what closes the finding.
//!
//! Split off `main.rs` so each file stays inside the three hundred line limit
//! this repository enforces on itself.

use crate::fixture::{
    domain_findings, sentence, service_row, stdout, Harness, UnitSpec, ACCOUNT, AGENT,
    AGENT_DAEMON_PATH, AGENT_PATH, AGENT_PROGRAM, ALWAYS_ON_HOST, INSTALL_COMMAND,
    INTERACTIVE_HOST, STREAM, STREAM_PATH, WELES,
};

/// `service list` prints the same sentence under the table, on the surface an
/// operator reads when the unit is missing from it.
#[test]
fn service_list_names_the_declared_domain_and_the_loadable_one() {
    let harness = Harness::new();
    harness.declare_registry(
        false,
        &UnitSpec::new(AGENT, AGENT_PATH),
        &UnitSpec::new(STREAM, STREAM_PATH),
    );
    harness.declare_beacon(ALWAYS_ON_HOST, &[WELES]);
    harness.declare_beacon(INTERACTIVE_HOST, &[STREAM]);

    let printed = stdout(&harness.stado(&["service", "list"]));

    assert!(
        printed.contains(&format!("declaration: {}", sentence())),
        "service list prints the declaration finding; got:\n{printed}"
    );
    // One line, for the one unit: the correctly declared daemon beside it and
    // the interactive host's agent produce nothing.
    assert_eq!(
        printed.matches("declaration: ").count(),
        1,
        "one declaration line; got:\n{printed}"
    );

    // The machine-readable half carries the same facts, for the dashboards
    // that read `--json` instead of the table.
    let out = harness.stado(&["service", "list", "--json"]);
    let row = service_row(&out, AGENT);
    let misdeclared = row
        .get("misdeclared_domain")
        .expect("the row carries the finding");
    assert_eq!(
        misdeclared
            .get("declared_domain")
            .and_then(serde_json::Value::as_str),
        Some("user")
    );
    assert_eq!(
        misdeclared
            .get("loadable_domain")
            .and_then(serde_json::Value::as_str),
        Some("system")
    );
    assert_eq!(
        misdeclared
            .get("daemon_path")
            .and_then(serde_json::Value::as_str),
        Some(AGENT_DAEMON_PATH)
    );
    assert_eq!(
        misdeclared
            .get("install_command")
            .and_then(serde_json::Value::as_str),
        Some(INSTALL_COMMAND)
    );
    assert_eq!(
        misdeclared
            .get("detail")
            .and_then(serde_json::Value::as_str),
        Some(sentence().as_str())
    );
    // The correctly declared daemon on the same host carries nothing.
    assert!(
        service_row(&out, WELES).get("misdeclared_domain").is_none(),
        "a system LaunchDaemon on an always-on host is not a finding"
    );
    assert!(
        service_row(&out, STREAM)
            .get("misdeclared_domain")
            .is_none(),
        "a user agent on an interactive host is not a finding"
    );
}

/// Correcting the declaration is what closes the finding, and the corrected
/// document — daemon path plus the program and arguments the unit runs — is
/// still a valid registry a `registry pull` round-trips.
#[test]
fn the_corrected_daemon_declaration_validates_and_closes_the_finding() {
    let harness = Harness::new();
    harness.declare_registry(
        false,
        &UnitSpec::new(AGENT, AGENT_DAEMON_PATH).running(AGENT_PROGRAM, &["agent", "--auto"]),
        &UnitSpec::new(STREAM, STREAM_PATH),
    );
    harness.declare_beacon(ALWAYS_ON_HOST, &[AGENT, WELES]);
    harness.declare_beacon(INTERACTIVE_HOST, &[STREAM]);

    // The document as `registry pull` hands it back, validated the way `push`
    // validates before it writes: a service entry carrying `program` and
    // `args` is registry-v2, not an unread key.
    let pulled = harness.stado(&["registry", "pull"]);
    assert!(pulled.status.success(), "registry pull answers");
    let pulled_path = harness.root().join("pulled.json");
    std::fs::write(&pulled_path, stdout(&pulled)).expect("write the pulled document");
    let validated = harness.stado(&[
        "registry",
        "validate",
        pulled_path.to_str().expect("a utf-8 path"),
    ]);
    assert!(
        validated.status.success(),
        "the corrected document validates; got:\n{}",
        String::from_utf8_lossy(&validated.stderr)
    );

    // And the finding is gone: the declaration and the host now agree.
    let doctor = harness.stado(&["registry", "doctor", "--json"]);
    assert!(
        domain_findings(&doctor).is_empty(),
        "a system LaunchDaemon on an always-on host is the right declaration"
    );
    let listed = stdout(&harness.stado(&["service", "list"]));
    assert!(
        !listed.contains("declaration: "),
        "service list prints no declaration finding; got:\n{listed}"
    );
    // The account the daemon has to keep running as is still named where an
    // operator reads it.
    assert!(
        INSTALL_COMMAND.contains(ACCOUNT),
        "the install command keeps the daemon on the owning account"
    );
}

/// The declaration finding is the cause of the `missing-plist` row for the
/// same unit, and only the cause survives: a beacon cannot report a unit
/// nothing ever loaded, and installing the plist where it is declared would
/// not change that.
#[test]
fn the_misdeclared_domain_replaces_the_missing_plist_symptom() {
    let harness = Harness::new();
    harness.declare_registry(
        false,
        &UnitSpec::new(AGENT, AGENT_PATH),
        &UnitSpec::new(STREAM, STREAM_PATH),
    );
    // The beacon knows the daemon and says nothing about the agent, exactly
    // as the always-on host's does.
    harness.declare_beacon(ALWAYS_ON_HOST, &[WELES]);
    harness.declare_beacon(INTERACTIVE_HOST, &[STREAM]);

    let out = harness.stado(&["registry", "doctor", "--json"]);
    let report: serde_json::Value =
        serde_json::from_str(&stdout(&out)).expect("registry doctor --json prints one object");
    let findings = report
        .get("findings")
        .and_then(serde_json::Value::as_array)
        .expect("a findings array");
    let about_agent: Vec<&serde_json::Value> = findings
        .iter()
        .filter(|finding| {
            finding
                .get("detail")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|detail| detail.contains(AGENT))
        })
        .collect();

    assert_eq!(about_agent.len(), 1, "one cause, one row: {about_agent:?}");
    assert_eq!(
        about_agent[0]
            .get("finding")
            .and_then(serde_json::Value::as_str),
        Some("misdeclared-domain")
    );
    assert_eq!(
        about_agent[0]
            .get("detail")
            .and_then(serde_json::Value::as_str),
        Some(sentence().as_str())
    );
}
