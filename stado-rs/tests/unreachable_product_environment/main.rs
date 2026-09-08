//! A product's declared environment that cannot reach the unit serving it.
//!
//! The shape every check in this pack had missed: a declaration nothing ever
//! confronts with the world. `registry doctor` compared the registry's
//! declared units against the beacons' one `state` word per unit, and
//! compared products against hosts only after resolving
//! `policy.targets.get(host)` — so the host absent from every target map was
//! the loop's skip condition rather than its finding.
//!
//! Every case here declares THIS machine: the registry names the host name
//! the kernel reports, the platform this build runs on, and no remote
//! destination, so the command resolves its current host to that target and
//! opens the unit files these cases write on this disk. Each finding is
//! produced by making the contradiction real in that document or on that
//! disk, and then withdrawn by taking the contradiction away again.
//!
//! Two conditions live here. A host no product target names cannot receive
//! the product's declared environment by any delivery path, and an adopted
//! stub records no program, no arguments and no environment, so the document
//! states nothing about what its unit starts with. The
//! unrecorded-declaration half is in `recorded.rs`; this file holds the
//! targeting half and the silences that keep either from firing everywhere.

mod document;
mod fixture;
mod recorded;

use document::{release_control, unit, ADOPTED, AUDIT, PRODUCT, UNRELATED, VAULT};
use document::{UNRECORDED, UNTARGETED};
use fixture::Fixture;

/// A host running a product's unit while no product target names it, so the
/// declared environment cannot reach it.
///
/// The row must name the host, the unit, the product and both variables: a
/// verdict an operator cannot act on is the same defect again.
#[test]
fn a_host_no_product_target_names_is_reported() {
    let fixture = Fixture::new();
    let path = fixture.write_plist(ADOPTED, "/bin/sh", &[]);
    fixture.declare(
        &[unit(ADOPTED, "", &path)],
        serde_json::json!({PRODUCT: "0.1.3", "stado": "0.13.46"}),
        release_control(&fixture.home(), &[]),
    );
    fixture.beacon(&[ADOPTED]);

    let row = fixture.only(UNTARGETED);
    let detail = row["detail"].as_str().expect("a detail sentence");
    for expected in [fixture.host(), ADOPTED, PRODUCT, AUDIT, VAULT] {
        assert!(
            detail.contains(expected),
            "row must name {expected}: {detail}"
        );
    }
    assert!(
        detail.contains(&format!(
            "release_control target map does not name {}",
            fixture.host()
        )),
        "row must state the target gap plainly: {detail}"
    );
    assert_eq!(
        row["subject"].as_str(),
        Some(fixture.host()),
        "the finding is about the host"
    );
}

/// The same document, once the product's target map names this host. The
/// declaration now has a delivery path, so the row must vanish: this is the
/// assertion that keeps the check from firing on every host in the fleet.
#[test]
fn naming_this_host_in_the_target_map_withdraws_the_row() {
    let fixture = Fixture::new();
    let path = fixture.write_plist(ADOPTED, "/bin/sh", &[]);
    let services = [unit(ADOPTED, "", &path)];
    let versions = serde_json::json!({PRODUCT: "0.1.3"});

    fixture.declare(
        &services,
        versions.clone(),
        release_control(&fixture.home(), &[]),
    );
    fixture.beacon(&[ADOPTED]);
    assert_eq!(
        fixture.findings(UNTARGETED).len(),
        1,
        "the unnamed host is the row this case then withdraws"
    );

    fixture.declare(
        &services,
        versions,
        release_control(&fixture.home(), &[fixture.host()]),
    );
    assert!(
        fixture.findings(UNTARGETED).is_empty(),
        "a named host has a delivery path: {:?}",
        fixture.details(UNTARGETED)
    );
}

/// A host that declares no version for the product is silent, and a unit that
/// does not name the product is silent.
///
/// Both witnesses are required precisely because a product declares an
/// environment on every host that runs it: keying off the policy alone would
/// fire everywhere, and a check that fires everywhere is switched off with
/// the defect still in place. The unrelated unit also pins that the product
/// is matched as a whole delimited segment of the label and never as a
/// substring.
#[test]
fn both_witnesses_are_required() {
    let undeclared = Fixture::new();
    let path = undeclared.write_plist(ADOPTED, "/bin/sh", &[]);
    let services = [unit(ADOPTED, "", &path)];
    undeclared.declare(
        &services,
        // No version declared for the product: the host never said it runs it.
        serde_json::json!({"stado": "0.13.46"}),
        release_control(&undeclared.home(), &[]),
    );
    undeclared.beacon(&[ADOPTED]);
    assert!(
        undeclared.findings(UNRECORDED).is_empty() && undeclared.findings(UNTARGETED).is_empty(),
        "a host declaring no version for the product said nothing to check: {:?}",
        undeclared.details(UNTARGETED)
    );
    // Declaring the version supplies the second witness, and both rows appear
    // against the very same unit file.
    undeclared.declare(
        &services,
        serde_json::json!({PRODUCT: "0.1.3", "stado": "0.13.46"}),
        release_control(&undeclared.home(), &[]),
    );
    assert_eq!(
        undeclared.findings(UNRECORDED).len(),
        1,
        "the declared version is the witness the first half was missing"
    );
    assert_eq!(undeclared.findings(UNTARGETED).len(), 1);

    let unrelated = Fixture::new();
    let path = unrelated.write_plist(UNRELATED, "/bin/sh", &[]);
    unrelated.declare(
        &[unit(UNRELATED, "", &path)],
        serde_json::json!({PRODUCT: "0.1.3"}),
        release_control(&unrelated.home(), &[]),
    );
    unrelated.beacon(&[UNRELATED]);
    assert!(
        unrelated.findings(UNRECORDED).is_empty() && unrelated.findings(UNTARGETED).is_empty(),
        "no unit here names the product: {:?}",
        unrelated.details(UNTARGETED)
    );
}

/// A product that declares no environment produces neither row.
///
/// There is nothing that could fail to reach the host, so both conditions are
/// vacuous and reporting them would be noise. Putting the two variables back
/// into the same document produces both rows again.
#[test]
fn a_product_declaring_no_environment_is_silent() {
    let fixture = Fixture::new();
    let path = fixture.write_plist(ADOPTED, "/bin/sh", &[]);
    let services = [unit(ADOPTED, "", &path)];
    let versions = serde_json::json!({PRODUCT: "0.1.3"});

    let mut control = release_control(&fixture.home(), &[]);
    control["products"][PRODUCT]["environment"] = serde_json::json!({});
    fixture.declare(&services, versions.clone(), control);
    fixture.beacon(&[ADOPTED]);
    assert!(
        fixture.findings(UNRECORDED).is_empty() && fixture.findings(UNTARGETED).is_empty(),
        "a product with no declared environment has nothing to fail to reach: {:?}",
        fixture.details(UNRECORDED)
    );

    fixture.declare(&services, versions, release_control(&fixture.home(), &[]));
    assert_eq!(fixture.findings(UNRECORDED).len(), 1);
    assert_eq!(fixture.findings(UNTARGETED).len(), 1);
}
