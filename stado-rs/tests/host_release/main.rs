//! A host's release state, read off the machine running this test.
//!
//! `wisent-ai/stado#566` deleted the old area for the right reason: it drove
//! the binary against a tempdir and asserted refusal wording, then called that
//! evidence for delivering a release. Here the registry host is this machine,
//! so `release host-state` reads the version the managed binary on this disk
//! actually prints, and the declaration it is compared against is written and
//! removed through the product's own verb and read back out of the registry
//! document.
//!
//! The delivering half is exercised exactly where it must refuse: with a
//! declaration older than the host, `--apply` has to stop, name the command
//! that moves the declaration, and leave the installed binary alone. That is
//! the apply path running for real, not a stand-in for it — and it is the only
//! part of it that can run here, because a delivery on this machine would
//! replace the operator's own Stado.
//!
//! Every sentence asserted below was copied from a live run on 2026-09-08.

mod fixture;
mod leased;
mod software;

use fixture::{
    installed_binary, installed_version, report, reported_binary, stderr, Fixture, BINARY,
    STALE_VERSION, TARGET,
};

#[test]
fn a_declaration_matching_this_machine_reads_back_as_in_sync() {
    let Some(version) = installed_version() else {
        // A machine without the managed binary is a real state, and the case
        // that covers it is the undeclared one below; there is nothing to
        // compare here, so fail loudly rather than pass quietly.
        panic!(
            "no managed binary at {} to read a version from",
            installed_binary().display()
        );
    };
    let fixture = Fixture::new();

    let declared = fixture.declare(&version);
    assert!(declared.status.success(), "{}", stderr(&declared));
    assert_eq!(
        fixture.declared_version().as_deref(),
        Some(version.as_str()),
        "the declaration has to land in the registry document"
    );

    let output = fixture.host_state(&[]);
    assert!(output.status.success(), "{}", stderr(&output));
    let report = report(&output);
    assert_eq!(report["target"], TARGET);
    assert_eq!(report["state"], "declared");
    assert_eq!(report["applied"], serde_json::json!(false));

    let binary = reported_binary(&report);
    assert_eq!(binary["binary"], BINARY);
    assert_eq!(binary["declared_version"], version.as_str());
    assert_eq!(
        binary["installed_version"],
        version.as_str(),
        "the report reads the version this machine's binary prints"
    );
    assert_eq!(binary["verdict"], "in-sync");
    assert_eq!(
        binary["root"],
        installed_binary().to_string_lossy().as_ref(),
        "the report names the binary it read"
    );
}

#[test]
fn a_stale_declaration_is_reported_as_the_host_being_ahead() {
    let Some(version) = installed_version() else {
        panic!("no managed binary to compare a stale declaration against");
    };
    let fixture = Fixture::new();
    assert!(fixture.declare(STALE_VERSION).status.success());

    let output = fixture.host_state(&[]);
    // A read that finds the host ahead of its declaration is itself a failure:
    // the registry is wrong about a machine, and a script reading only the exit
    // status has to hear that.
    assert_eq!(
        output.status.code(),
        Some(1),
        "a stale declaration is a failing read: {}",
        stderr(&output)
    );
    assert!(
        stderr(&output)
            .contains("declared binary/binaries run a version NEWER than the registry declares"),
        "{}",
        stderr(&output)
    );
    let report = report(&output);
    let binary = reported_binary(&report);

    assert_eq!(binary["declared_version"], STALE_VERSION);
    assert_eq!(binary["installed_version"], version.as_str());
    assert_eq!(binary["verdict"], "host-ahead");
    let detail = binary["detail"].as_str().expect("a stated detail");
    assert!(
        detail.contains(&format!(
            "the host runs {version}, newer than the declared {STALE_VERSION}"
        )),
        "{detail}"
    );
    assert!(
        detail.contains("the declaration is stale, not the host"),
        "{detail}"
    );
}

#[test]
fn apply_refuses_to_downgrade_an_ahead_host_and_changes_nothing() {
    let Some(version) = installed_version() else {
        panic!("no managed binary to protect from a downgrade");
    };
    let fixture = Fixture::new();
    assert!(fixture.declare(STALE_VERSION).status.success());

    let output = fixture.host_state(&["--apply"]);
    assert_eq!(
        output.status.code(),
        Some(1),
        "a refused delivery is a failure: {}",
        stderr(&output)
    );
    let refusal = stderr(&output);
    assert!(
        refusal.contains(&format!(
            "{BINARY}: runs {version}, newer than the declared {STALE_VERSION} — refused to \
             downgrade the host; move the declaration instead: stado release declare-version \
             --host {TARGET} --binary {BINARY} --version {version}"
        )),
        "the refusal has to name the command that moves the declaration: {refusal}"
    );
    assert!(
        refusal.contains("host-ahead binary/binaries were refused rather than downgraded"),
        "{refusal}"
    );

    // Nothing was delivered: the binary on this machine still prints the same
    // version, and the stale declaration is still the stale one.
    assert_eq!(installed_version().as_deref(), Some(version.as_str()));
    assert_eq!(fixture.declared_version().as_deref(), Some(STALE_VERSION));
}

#[test]
fn unsetting_the_declaration_leaves_the_host_undeclared() {
    let fixture = Fixture::new();
    assert!(fixture.declare(STALE_VERSION).status.success());
    assert_eq!(fixture.declared_version().as_deref(), Some(STALE_VERSION));

    let unset = fixture.unset();
    assert!(unset.status.success(), "{}", stderr(&unset));
    assert_eq!(
        fixture.declared_version(),
        None,
        "the declaration has to leave the registry document"
    );

    let output = fixture.host_state(&[]);
    assert!(output.status.success(), "{}", stderr(&output));
    let report = report(&output);
    assert_eq!(report["state"], "undeclared");
    assert_eq!(
        report["binaries"].as_array().map(Vec::len),
        Some(0),
        "an undeclared host reports no binaries: {report}"
    );
}

#[test]
fn a_host_outside_the_registry_is_refused() {
    let fixture = Fixture::new();
    let output = fixture.stado(&["release", "host-state", "--host", "nowhere", "--json"]);

    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("target 'nowhere' is not in the canonical registry"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn a_binary_the_host_never_declared_is_refused_with_the_command_that_declares_it() {
    let fixture = Fixture::new();
    let output = fixture.host_state(&["--binary", "invented"]);

    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(
        stderr(&output).contains(&format!(
            "{TARGET} declares no invented version; add it to targets[].managed_versions with \
             `stado release declare-version --host {TARGET} --binary invented --version X.Y.Z`"
        )),
        "{}",
        stderr(&output)
    );
}

#[test]
fn promoting_a_version_refuses_without_a_canonical_release_source() {
    let fixture = Fixture::new();
    let output = fixture.stado(&[
        "release",
        "promote-version",
        "--host",
        TARGET,
        "--binary",
        BINARY,
        "--version",
        "9.9.9",
        "--json",
    ]);

    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("STADO_API_URL is required for canonical release reads"),
        "a promotion cannot invent a published version: {}",
        stderr(&output)
    );
    assert_eq!(
        fixture.declared_version(),
        None,
        "a refused promotion writes no declaration"
    );
}
