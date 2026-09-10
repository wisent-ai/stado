//! Declared GitHub runners, observed on the machine running this test.
//!
//! `wisent-ai/stado#566` deleted the old area and the reason was right: it
//! rendered an installer program and asserted its text, then called that
//! evidence for installing a runner. Here the registry host is this machine,
//! so `runner status` and `runner diagnostics` take the current-host path and
//! execute this machine's own service tooling, and every reported fact is
//! confronted with this machine's filesystem: whether a runner is configured,
//! where its root is, and whether its diagnostic log exists.
//!
//! Registration is never performed. `runner install` registers a live runner
//! in the organization and a test has no business doing that — tonight a
//! second runner on one host was already one accident away. What runs instead
//! is the guard: install refuses an undeclared profile and a host outside the
//! registry, and the cases prove nothing was created on the way to the
//! refusal.
//!
//! Every sentence asserted below was copied from a live run on 2026-09-08.

mod fixture;
mod identity;
mod memory;

use std::path::Path;

use fixture::{
    platform, report, runner_is_configured, stderr, Fixture, PROFILE_DECLARATION, TARGET,
};

#[test]
fn status_reads_this_machine_and_its_installed_claim_matches_the_filesystem() {
    let fixture = Fixture::new();
    let declared: Vec<String> = fixture
        .declared_profiles()
        .iter()
        .map(|profile| profile["name"].as_str().expect("a name").to_string())
        .collect();

    let output = fixture.status(&[]);
    assert!(output.status.success(), "{}", stderr(&output));
    let status_report = report(&output);
    let reported = status_report["profiles"]
        .as_array()
        .expect("the status report covers the declared profiles");
    assert_eq!(
        reported.len(),
        declared.len(),
        "every declared profile is reported: {status_report}"
    );

    for entry in reported {
        let name = entry["profile"].as_str().expect("a reported profile name");
        assert!(declared.iter().any(|profile| profile == name), "{name}");
        assert_eq!(entry["target"], TARGET);
        assert_eq!(
            entry["platform"],
            platform(),
            "the read reports the platform this test is running on"
        );

        // Where the runner would live is only known from the host read, so the
        // installed claim is checked against that same host's disk.
        let diagnostics = fixture.diagnostics(name);
        assert!(diagnostics.status.success(), "{}", stderr(&diagnostics));
        let root = report(&diagnostics)["runner_root"]
            .as_str()
            .expect("the diagnostics name the runner root")
            .to_string();
        assert_eq!(
            entry["installed"],
            serde_json::json!(runner_is_configured(&root)),
            "the installed claim for {name} disagrees with {root}"
        );
    }
}

#[test]
fn diagnostics_report_the_log_this_machine_actually_has() {
    let fixture = Fixture::new();
    let profiles = fixture.declared_profiles();
    let name = profiles[0]["name"].as_str().expect("a profile name");

    let output = fixture.diagnostics(name);
    assert!(output.status.success(), "{}", stderr(&output));
    let report = report(&output);

    assert_eq!(report["profile"], name);
    assert_eq!(report["platform"], platform());
    let root = report["runner_root"].as_str().expect("a runner root");
    assert!(
        Path::new(root).is_absolute(),
        "the runner root is a path on this machine: {root}"
    );

    let standard_error = report["standard_error"]
        .as_str()
        .expect("the diagnostics name the error log they read");
    let log = report["log"].as_str().expect("a log verdict");
    if Path::new(standard_error).is_file() {
        assert_ne!(
            log, "none",
            "a log that exists was reported as absent: {standard_error}"
        );
    } else {
        assert_eq!(
            log, "none",
            "a log that does not exist was reported as read: {standard_error}"
        );
    }
}

#[test]
fn a_host_outside_the_registry_is_refused_by_every_verb() {
    let fixture = Fixture::new();
    let sentence = "nowhere-host declares no host target; add it to the canonical fleet registry";

    let status = fixture.stado(&["runner", "status", "nowhere-host", "--json"]);
    assert_eq!(status.status.code(), Some(1), "{}", stderr(&status));
    assert!(stderr(&status).contains(sentence), "{}", stderr(&status));

    let install = fixture.stado(&[
        "runner",
        "install",
        "--profile",
        "precheck",
        "nowhere-host",
        "--json",
    ]);
    assert_eq!(install.status.code(), Some(1), "{}", stderr(&install));
    assert!(stderr(&install).contains(sentence), "{}", stderr(&install));
}

#[test]
fn an_undeclared_profile_is_refused_before_anything_is_installed() {
    let fixture = Fixture::new();
    let sentence =
        format!("runner profile 'invented' is not declared; add it to {PROFILE_DECLARATION}");
    let root = "/Users/Shared/stado-invented-runner";

    let status = fixture.status(&["--profile", "invented"]);
    assert_eq!(status.status.code(), Some(1), "{}", stderr(&status));
    assert!(stderr(&status).contains(&sentence), "{}", stderr(&status));

    let install = fixture.stado(&[
        "runner",
        "install",
        "--profile",
        "invented",
        TARGET,
        "--json",
    ]);
    assert_eq!(install.status.code(), Some(1), "{}", stderr(&install));
    assert!(stderr(&install).contains(&sentence), "{}", stderr(&install));
    assert!(
        !Path::new(root).exists(),
        "the refusal created a runner root: {root}"
    );
}
