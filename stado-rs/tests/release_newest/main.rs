//! One command for the whole workspace, proved through the real binary.
//!
//! Releasing used to be one `stado release submit --source <path> --version
//! <version>` per product, with a version the caller typed even though the
//! pipeline reads `version_source` out of the committed manifest and refuses
//! a `--version` that disagrees with it. `stado release newest` reads the
//! workspace instead: every product checkout, the commit it stands on, the
//! version that commit declares, and whether that version is already
//! published.
//!
//! Every case runs `CARGO_BIN_EXE_stado` against real Git checkouts in the
//! area beside this file. Nothing is simulated.

mod area;
mod changes;

use area::{releasing_manifest, silent_manifest, Area};
use serde_json::Value;
use std::process::Output;

const REFUSED_EXIT: i32 = 1;
/// A full Git object name, which is what a plan names as the commit.
const COMMIT_LENGTH: usize = 40;

fn document(output: &Output) -> Value {
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    serde_json::from_str(&stdout).unwrap_or_else(|error| {
        panic!(
            "expected one JSON document, got {error}\nstdout: {stdout}\nstderr: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn entry<'a>(report: &'a Value, product: &str) -> &'a Value {
    report["products"]
        .as_array()
        .expect("the report lists products")
        .iter()
        .find(|entry| entry["product"] == product)
        .unwrap_or_else(|| panic!("{product} is missing from {report}"))
}

/// The whole point: one reading of the workspace names every product, what
/// would be released, and why the rest would not.
#[test]
fn one_reading_names_every_product_and_what_would_be_released() {
    let area = Area::new("workspace");
    area.checkout(
        "releasing-product",
        &releasing_manifest("releasing-product"),
        Some(("package.json", "{\"version\": \"1.4.2\"}")),
    );
    area.checkout(
        "silent-product",
        &silent_manifest("silent-product", "this repository ships nothing"),
        None,
    );
    let dirty = area.checkout(
        "dirty-product",
        &releasing_manifest("dirty-product"),
        Some(("package.json", "{\"version\": \"0.9.0\"}")),
    );
    std::fs::write(dirty.join("package.json"), "{\"version\": \"0.9.1\"}")
        .expect("leave an uncommitted change behind");
    // Another session's unfinished work in the one shared checkout, including
    // an edit to the version file that keeps the version: it is not what the
    // commit declares, so the commit is still released without it.
    let busy = area.checkout(
        "busy-product",
        &releasing_manifest("busy-product"),
        Some(("package.json", "{\"version\": \"2.0.1\"}")),
    );
    std::fs::write(busy.join("notes.txt"), "unfinished").expect("leave untracked work behind");
    std::fs::write(
        busy.join("package.json"),
        "{\"version\": \"2.0.1\", \"private\": true}",
    )
    .expect("leave a version-preserving edit behind");

    let planned = area.plan(&[]);
    assert!(
        planned.status.success(),
        "the plan failed: {}",
        String::from_utf8_lossy(&planned.stderr)
    );
    let report = document(&planned);

    let releasing = entry(&report, "releasing-product");
    assert_eq!(releasing["standing"], "releasable");
    assert_eq!(
        releasing["version"], "1.4.2",
        "the version comes from the checkout, never from the caller: {releasing}"
    );
    assert_eq!(
        releasing["commit"].as_str().unwrap_or_default().len(),
        COMMIT_LENGTH,
        "the commit the checkout stands on is named in full: {releasing}"
    );

    let silent = entry(&report, "silent-product");
    assert_eq!(silent["standing"], "declares_no_releases");
    assert_eq!(silent["reason"], "this repository ships nothing");

    let dirty = entry(&report, "dirty-product");
    assert_eq!(
        dirty["standing"], "unreadable",
        "a checkout with uncommitted work cannot be released: {dirty}"
    );
    assert!(
        dirty["refusal"]
            .as_str()
            .unwrap_or_default()
            .contains("clean committed Git tree"),
        "the refusal says what is wrong with it: {dirty}"
    );

    let busy = entry(&report, "busy-product");
    assert_eq!(
        busy["standing"], "releasable",
        "work that declares nothing does not hold the commit back: {busy}"
    );
    assert_eq!(busy["version"], "2.0.1", "the committed version: {busy}");
    assert_eq!(
        busy["uncommitted"], 2,
        "the plan says what it leaves out: {busy}"
    );
    assert_eq!(
        releasing["uncommitted"], 0,
        "a clean checkout leaves nothing out"
    );
}

/// A product nobody can find is a refusal. A release that silently did
/// nothing reads exactly like a release that worked.
#[test]
fn a_product_the_workspace_does_not_hold_is_refused() {
    let area = Area::new("unknown-product");
    area.checkout(
        "releasing-product",
        &releasing_manifest("releasing-product"),
        Some(("package.json", "{\"version\": \"1.0.0\"}")),
    );

    let planned = area.plan(&["--product", "no-such-product"]);
    assert_eq!(planned.status.code(), Some(REFUSED_EXIT));
    assert!(
        String::from_utf8_lossy(&planned.stderr).contains("no-such-product"),
        "the refusal names the product asked for: {}",
        String::from_utf8_lossy(&planned.stderr)
    );
}

/// An empty workspace is refused rather than reported as a successful release
/// of nothing.
#[test]
fn a_workspace_with_no_product_is_refused() {
    let area = Area::new("empty");
    let planned = area.plan(&[]);
    assert_eq!(planned.status.code(), Some(REFUSED_EXIT));
    let refusal = String::from_utf8_lossy(&planned.stderr).into_owned();
    assert!(
        refusal.contains(".wisent-release.json"),
        "the refusal says what it looked for: {refusal}"
    );
}

/// Selecting one product reads that product only, and the selection does not
/// change what is said about it.
#[test]
fn one_selected_product_is_planned_alone() {
    let area = Area::new("selected");
    area.checkout(
        "first-product",
        &releasing_manifest("first-product"),
        Some(("package.json", "{\"version\": \"2.0.0\"}")),
    );
    area.checkout(
        "second-product",
        &releasing_manifest("second-product"),
        Some(("package.json", "{\"version\": \"3.0.0\"}")),
    );

    let planned = area.plan(&["--product", "second-product"]);
    assert!(
        planned.status.success(),
        "the plan failed: {}",
        String::from_utf8_lossy(&planned.stderr)
    );
    let report = document(&planned);
    let products = report["products"].as_array().expect("products are listed");
    assert_eq!(products.len(), 1, "only the selected product: {report}");
    assert_eq!(products[0]["product"], "second-product");
    assert_eq!(products[0]["version"], "3.0.0");
}
