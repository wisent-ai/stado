//! Release declarations are isolated registry writes; actual delivery belongs
//! to the disposable-account story in `leased`. No case assumes that the
//! operator's own executable happens to be attested or replaces that file.

mod fixture;
mod leased;
mod software;

use fixture::{report, stderr, Fixture, BINARY, STALE_VERSION, TARGET};

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
