//! `registry doctor` keeps the two release mechanisms separate.
//!
//! `release_control.products.<product>.desired` is the version authority for
//! release-agent products. `targets[].managed_versions` is the authority only
//! for products in the compiled `host release` catalog. Requiring both creates
//! two desired versions and recommends commands the catalog refuses.
//!
//! Every case declares THIS machine — the host name the kernel reports, the
//! platform this build runs on, no remote destination — and creates the
//! delivered program it talks about as a real file under a temporary home, so
//! the verdicts below are about a host that exists and a tree that is on this
//! disk. Each finding is produced by writing the contradicting declaration
//! and then withdrawn by writing the declaration that resolves it.

mod fixture;

use fixture::{platform, release_control, unit, Fixture};
use fixture::{ARBITRARY, CATALOG_PRODUCT, LABEL_STAGED, LEGACY, MANAGED, VERSION_FINDING};

/// A product release control owns carries its own desired version, so the
/// host must not be told to declare a second one — and its legacy launchd
/// unit is scheduled for bootout rather than for liveness.
///
/// Dropping the release-control block from the same document leaves the same
/// unit inside the compiled catalog with nothing declaring its version, which
/// is exactly the row this case proves must not fire while control owns it.
#[test]
fn release_control_owns_its_version_and_legacy_unit_liveness() {
    let fixture = Fixture::new();
    let program = fixture.delivery_program("skarbiec", "bin/start-with-vault");
    let services = [unit(&fixture.home(), LEGACY, &program)];
    fixture.declare(
        &services,
        serde_json::json!({}),
        Some(release_control(fixture.host(), &fixture.home())),
    );
    fixture.beacon(&[]);

    assert!(
        fixture.findings(VERSION_FINDING).is_empty(),
        "a release-control product must not require a duplicate managed version: {:?}",
        fixture.details(VERSION_FINDING)
    );
    assert!(
        fixture.findings("missing-plist").is_empty(),
        "the release agent intentionally removes the legacy unit: {:?}",
        fixture.details("missing-plist")
    );
    assert!(
        fixture.findings("unit-not-active").is_empty(),
        "the legacy unit is not a liveness subject: {:?}",
        fixture.details("unit-not-active")
    );

    // Withdraw the ownership and the same unit becomes an undeclared
    // compiled-catalog delivery, unit liveness included.
    fixture.declare(&services, serde_json::json!({}), None);
    let detail = fixture.details(VERSION_FINDING).pop().expect("one row");
    assert!(
        detail.contains("--binary skarbiec"),
        "an unowned catalog product is measured against managed_versions: {detail}"
    );
    assert_eq!(
        fixture.findings("missing-plist").len(),
        1,
        "a unit nothing boots out is a liveness subject: {:?}",
        fixture.details("missing-plist")
    );
}

/// A unit delivered from the compiled catalog's own tree whose host declares
/// no version for it.
///
/// The recommended command has to be one the catalog accepts, because the row
/// that recommends a refused command is the row an operator learns to ignore.
#[test]
fn a_compiled_managed_product_without_a_declared_version_is_reported() {
    let fixture = Fixture::new();
    let program = fixture.delivery_program(CATALOG_PRODUCT, CATALOG_PRODUCT);
    fixture.declare(
        &[unit(&fixture.home(), MANAGED, &program)],
        serde_json::json!({}),
        None,
    );
    fixture.beacon(&[MANAGED]);

    let row = fixture.only(VERSION_FINDING);
    let detail = row["detail"].as_str().expect("a detail sentence");
    assert!(detail.contains(MANAGED), "names the unit: {detail}");
    assert!(detail.contains(&program), "names the program: {detail}");
    assert!(
        detail.contains(&format!("--binary {CATALOG_PRODUCT}")),
        "recommends a command the catalog accepts: {detail}"
    );
    assert_eq!(row["subject"].as_str(), Some(fixture.host()));
}

/// Declaring the version in the same document withdraws the row, and
/// unsetting it brings the row back.
///
/// This is the reversal that keeps the check honest: the finding is about the
/// `managed_versions` entry and nothing else in the document moved.
#[test]
fn declaring_the_version_withdraws_the_row() {
    let fixture = Fixture::new();
    let program = fixture.delivery_program(CATALOG_PRODUCT, CATALOG_PRODUCT);
    let services = [unit(&fixture.home(), MANAGED, &program)];
    fixture.declare(
        &services,
        serde_json::json!({CATALOG_PRODUCT: "0.15.9"}),
        None,
    );
    fixture.beacon(&[MANAGED]);
    assert!(
        fixture.findings(VERSION_FINDING).is_empty(),
        "a declared version is the whole answer: {:?}",
        fixture.details(VERSION_FINDING)
    );

    fixture.declare(&services, serde_json::json!({}), None);
    assert_eq!(
        fixture.findings(VERSION_FINDING).len(),
        1,
        "removing the declaration restores the gap"
    );
}

/// A tree staged under the unit's own label still carries the catalog's
/// version contract, because the file it runs is the catalog binary.
///
/// The tree name is not a product here, so the program's own file name is the
/// witness, and the row must name the product `host declare-version` accepts
/// rather than the label the tree happens to be staged under.
#[test]
fn a_label_staged_tree_maps_to_the_product_its_program_is() {
    let fixture = Fixture::new();
    let program = fixture.delivery_program(LABEL_STAGED, CATALOG_PRODUCT);
    fixture.declare(
        &[unit(&fixture.home(), LABEL_STAGED, &program)],
        serde_json::json!({}),
        None,
    );
    fixture.beacon(&[LABEL_STAGED]);

    let detail = fixture.details(VERSION_FINDING).pop().expect("one row");
    assert!(
        detail.contains(&format!("as managed product {CATALOG_PRODUCT:?}")),
        "the row must name the catalog product, not the staging label: {detail}"
    );
    assert!(
        detail.contains(&format!("--binary {CATALOG_PRODUCT}")),
        "the recommended command must name the catalog product: {detail}"
    );

    // Declaring the version the row asks for withdraws it; declaring a
    // version under the staging label instead does not, because that label is
    // not a product the catalog knows.
    fixture.declare(
        &[unit(&fixture.home(), LABEL_STAGED, &program)],
        serde_json::json!({LABEL_STAGED: "0.15.9"}),
        None,
    );
    assert_eq!(
        fixture.findings(VERSION_FINDING).len(),
        1,
        "a version declared under a name the catalog refuses answers nothing"
    );
    fixture.declare(
        &[unit(&fixture.home(), LABEL_STAGED, &program)],
        serde_json::json!({CATALOG_PRODUCT: "0.15.9"}),
        None,
    );
    assert!(fixture.findings(VERSION_FINDING).is_empty());
}

/// An arbitrary tree installed by `service update` is not a managed-product
/// declaration and gets no invented semver contract.
///
/// Neither the tree name nor the program's file name is a catalog product, so
/// there is no command `host declare-version` would accept and therefore no
/// row worth printing. Restaging the same unit under the catalog product's
/// own tree is what makes the contract real.
#[test]
fn an_arbitrary_service_update_tree_gets_no_invented_semver_contract() {
    let fixture = Fixture::new();
    let program = fixture.delivery_program("weles-admission", "weles-api-launcher");
    fixture.declare(
        &[unit(&fixture.home(), ARBITRARY, &program)],
        serde_json::json!({}),
        None,
    );
    fixture.beacon(&[ARBITRARY]);
    assert!(
        fixture.findings(VERSION_FINDING).is_empty(),
        "service update tracks its artifact without pretending it is a \
         host-release product: {:?}",
        fixture.details(VERSION_FINDING)
    );

    let catalog = fixture.delivery_program(CATALOG_PRODUCT, "weles-api-launcher");
    assert!(
        catalog.contains(&format!("/.stado/services/{CATALOG_PRODUCT}/current/")),
        "the delivery tree really is the shape the reader keys off: {catalog}"
    );
    assert!(catalog.contains(platform()), "and this build's platform");
    fixture.declare(
        &[unit(&fixture.home(), ARBITRARY, &catalog)],
        serde_json::json!({}),
        None,
    );
    assert_eq!(
        fixture.findings(VERSION_FINDING).len(),
        1,
        "the same unit staged under a catalog product does carry the contract"
    );
}
