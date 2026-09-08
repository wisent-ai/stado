//! The half of the incident the registry could have answered from its own
//! document: an adopted stub that records no program, no arguments and no
//! environment, against a product policy that declares one.
//!
//! Every case here runs against the machine the suite runs on, so the unit
//! file the record points at is a file on this disk. That is what lets a
//! verdict about what a unit carries be checked by writing the variable into
//! the unit, and a verdict about a unit that is not there be checked by
//! removing the file and putting it back.

use crate::document::{release_control, unit, ADOPTED, AUDIT, LAUNCHER, PRODUCT, RECORDED, VAULT};
use crate::document::{UNRECORDED, UNTARGETED};
use crate::fixture::Fixture;

/// An adopted stub whose unit carries nothing, against a policy that declares
/// two variables.
///
/// The row must name the variable the unit does not carry. Reporting "drift"
/// here, or reporting the declared set without saying which of it is absent,
/// leaves the operator exactly where the unpinned journal left them.
#[test]
fn an_adopted_unit_missing_a_declared_variable_names_it() {
    let fixture = Fixture::new();
    // The incident's plist exactly: an empty EnvironmentVariables dict and a
    // launcher that pins nothing.
    let path = fixture.write_plist(ADOPTED, LAUNCHER, &[]);
    fixture.declare(
        &[unit(ADOPTED, "", &path)],
        serde_json::json!({PRODUCT: "0.1.3"}),
        // Named, so only the unrecorded-declaration half is under test here.
        release_control(&fixture.home(), &[fixture.host()]),
    );
    fixture.beacon(&[ADOPTED]);

    let row = fixture.only(UNRECORDED);
    let detail = row["detail"].as_str().expect("a detail sentence");
    for expected in [
        fixture.host(),
        ADOPTED,
        PRODUCT,
        AUDIT,
        &path.display().to_string(),
    ] {
        assert!(
            detail.contains(expected),
            "row must name {expected}: {detail}"
        );
    }
    assert!(
        detail.contains("no environment variables at all"),
        "row must say what the unit actually carries: {detail}"
    );
    assert!(
        detail.contains("recording no program"),
        "row must say the record itself is empty: {detail}"
    );
    assert!(
        !detail.contains("was not read") && !detail.contains("does not exist on this host"),
        "the unit was readable here, so it must not be reported unread: {detail}"
    );
}

/// A unit that carries only one of the two declared variables. The row must
/// name the absent one and not the present one, because the operator's next
/// action is to pin exactly that variable — and once the unit carries both,
/// the row must stop naming any variable as absent.
#[test]
fn only_the_variables_the_unit_lacks_are_named_as_missing() {
    let fixture = Fixture::new();
    let vault_file = fixture.home().join(".stado/skarbiec.vault.json");
    let vault_file = vault_file.display().to_string();
    let path = fixture.write_plist(ADOPTED, LAUNCHER, &[(VAULT, &vault_file)]);
    fixture.declare(
        &[unit(ADOPTED, "", &path)],
        serde_json::json!({PRODUCT: "0.1.3"}),
        release_control(&fixture.home(), &[fixture.host()]),
    );
    fixture.beacon(&[ADOPTED]);

    let detail = fixture.details(UNRECORDED).pop().expect("one row");
    assert!(
        detail.contains(&format!(
            "{AUDIT} is declared and the unit does not carry it"
        )),
        "the absent variable must be named as absent: {detail}"
    );
    assert!(
        detail.contains(&format!("carries {VAULT}")),
        "the variable the unit does hold must be reported as held: {detail}"
    );

    // Pin the second variable in the unit itself. The record is still empty,
    // so the row stays; what changes is the clause about what is missing.
    let audit_file = fixture.home().join(".stado/skarbiec.audit.jsonl");
    let audit_file = audit_file.display().to_string();
    fixture.write_plist(
        ADOPTED,
        LAUNCHER,
        &[(VAULT, &vault_file), (AUDIT, &audit_file)],
    );
    let detail = fixture.details(UNRECORDED).pop().expect("one row");
    assert!(
        detail.contains("Every declared variable is present, so only the record is missing"),
        "a unit that carries both must not be told a variable is absent: {detail}"
    );
    assert!(
        !detail.contains("does not carry"),
        "nothing is absent from this unit any more: {detail}"
    );
}

/// A record that points at a unit file this host does not hold.
///
/// The read is attempted — this IS the host the command ran on — and comes
/// back with nothing, which is a different fact from a unit on another host
/// that was never opened. Reporting the second for the first tells an
/// operator standing on the affected machine that the machine is not the one
/// the command ran on, and sends them to a command that reads the very same
/// absent file. The case reads the unit while it is there, then takes the
/// file away and reads the same record again.
#[test]
fn a_record_pointing_at_an_absent_unit_file_says_so() {
    let fixture = Fixture::new();
    let path = fixture.write_plist(ADOPTED, LAUNCHER, &[]);
    fixture.declare(
        &[unit(ADOPTED, "", &path)],
        serde_json::json!({PRODUCT: "0.1.3"}),
        release_control(&fixture.home(), &[fixture.host()]),
    );
    fixture.beacon(&[ADOPTED]);

    let detail = fixture.details(UNRECORDED).pop().expect("one row");
    assert!(
        detail.contains(&format!(
            "The unit file {} on this host carries",
            path.display()
        )),
        "a unit file that is there is read: {detail}"
    );

    // Take the file away. The record still points at it, and the same pass
    // still runs on the host that owns it.
    fixture.remove_plist(ADOPTED);
    let detail = fixture.details(UNRECORDED).pop().expect("one row");
    assert!(
        detail.contains(&format!(
            "The unit file {} does not exist on this host",
            path.display()
        )),
        "an absent unit file must be reported absent: {detail}"
    );
    assert!(
        !detail.contains("is not the host this ran on"),
        "this IS the host the command ran on: {detail}"
    );
    assert!(
        !detail.contains("no environment variables at all"),
        "an unread unit must never be reported as carrying nothing: {detail}"
    );
    assert!(
        detail.contains(&format!("stado service adopt {} {ADOPTED}", fixture.host())),
        "the row must still name the command that records the declaration: {detail}"
    );
}

/// A unit whose declaration IS recorded produces no unrecorded row.
///
/// `service deploy` records the program and args it rendered the unit from,
/// so there is something in the document to diff. Firing here would report
/// every properly declared service in the fleet. Emptying the recorded
/// program in that same document brings the row back.
#[test]
fn a_unit_whose_declaration_is_recorded_is_silent() {
    let fixture = Fixture::new();
    let path = fixture.write_plist(RECORDED, LAUNCHER, &[]);
    let versions = serde_json::json!({PRODUCT: "0.1.3"});
    fixture.declare(
        &[unit(RECORDED, LAUNCHER, &path)],
        versions.clone(),
        release_control(&fixture.home(), &[fixture.host()]),
    );
    fixture.beacon(&[RECORDED]);
    assert!(
        fixture.findings(UNRECORDED).is_empty(),
        "a recorded declaration has something to diff: {:?}",
        fixture.details(UNRECORDED)
    );
    assert!(fixture.findings(UNTARGETED).is_empty());

    // The same unit, adopted as a bare path instead: now the document states
    // nothing about what it starts with.
    fixture.declare(
        &[unit(RECORDED, "", &path)],
        versions,
        release_control(&fixture.home(), &[fixture.host()]),
    );
    assert_eq!(
        fixture.findings(UNRECORDED).len(),
        1,
        "an empty recorded program is the whole condition"
    );
}
