//! A product that publishes no release declares its quality gates in a
//! top-level `quality` list. `stado quality format` runs its `format` gate on
//! the real source and leaves it formatted; the installation check refuses,
//! since nothing installs such a product; a manifest whose list holds no
//! `format` gate is refused naming the list.

use super::{Deployment, MANIFEST};
use serde_json::{json, Value};
use std::fs;

const PRODUCT: &str = "quality-non-release-test";

/// A committed checkout of a product that publishes no release, holding
/// `source` and declaring `quality`.
fn checkout(deployment: &mut Deployment, name: &str, source: &str, quality: Value) -> String {
    let checkout = deployment.root.join(name);
    fs::create_dir_all(checkout.join("src")).unwrap();
    deployment.git(&checkout, &["init", "--quiet", "--initial-branch=main"]);
    let own: Value =
        serde_json::from_slice(&fs::read(deployment.repository.join(MANIFEST)).unwrap()).unwrap();
    fs::write(checkout.join("src/lib.rs"), source).unwrap();
    let manifest = json!({
        "schema_version": own["schema_version"],
        "product": PRODUCT,
        "releases": false,
        "reason": "Installed from its source checkout; it publishes no release.",
        "quality": quality,
    });
    fs::write(
        checkout.join(MANIFEST),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    deployment.git(&checkout, &["add", "src/lib.rs", MANIFEST]);
    deployment.git(&checkout, &["commit", "--quiet", "-m", "declare the gates"]);
    checkout.to_string_lossy().into_owned()
}

fn said(output: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn a_product_without_releases_is_formatted_by_its_declared_gate() {
    let mut deployment = Deployment::start();
    let initialised = deployment.run(&["config", "init"]);
    assert!(
        initialised.status.success(),
        "config init: {}",
        said(&initialised)
    );

    let unformatted = checkout(
        &mut deployment,
        "non-release-format",
        "pub fn answer()->bool{true}\n",
        json!([{"name": "style", "kind": "format", "argv": ["rustfmt", "--check", "src/lib.rs"]}]),
    );
    let formatted = deployment.run(&["quality", "format", "--root", &unformatted]);
    assert!(
        formatted.status.success(),
        "the declared format gate did not run: {}",
        said(&formatted)
    );
    assert_eq!(
        fs::read_to_string(std::path::Path::new(&unformatted).join("src/lib.rs")).unwrap(),
        "pub fn answer() -> bool {\n    true\n}\n",
        "the source was not left formatted: {}",
        said(&formatted)
    );

    let installed = deployment.run(&["quality", "check", "--root", &unformatted]);
    assert!(!installed.status.success(), "{}", said(&installed));
    assert!(
        said(&installed).contains("declares releases:false, so no installation builds it"),
        "{}",
        said(&installed)
    );

    let undeclared = checkout(
        &mut deployment,
        "non-release-no-format",
        "pub fn answer() -> bool {\n    true\n}\n",
        json!([]),
    );
    let refused = deployment.run(&["quality", "format", "--root", &undeclared]);
    assert!(!refused.status.success(), "{}", said(&refused));
    assert!(
        said(&refused).contains("declares releases:false and no gate of kind format"),
        "{}",
        said(&refused)
    );
    deployment.save();
}
