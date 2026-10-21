//! `stado quality check` over a TypeScript web product refuses a missing
//! typecheck script, a dependency unavailable to `npm ci`, and a declared
//! `tsc --noEmit` script that finds an incompatible type. Neither failed
//! command sends the operator to a formatter; repaired source passes.
//!
//! One isolated checkout with its own origin and a web platform whose gate is
//! `stado web quality`, run by the real `stado` this test builds (its
//! directory leads PATH, so the gate's `stado` is the same binary). The
//! project has a tsconfig.json and a build script, and npm writes its lock.
//! The test commits each transition and inspects the gate's observed result.
mod fixtures;

use fixtures::{Deployment, PRODUCT};
use serde_json::{json, Value};
use std::fs;

#[test]
fn a_web_gate_names_install_and_typecheck_failures_then_passes_the_repair() {
    let mut deployment = Deployment::start();
    let initialised = deployment.run(&["config", "init"]);
    assert!(
        initialised.status.success(),
        "config init: {}",
        String::from_utf8_lossy(&initialised.stderr)
    );
    let checkout = deployment.checkout();
    let root = checkout.to_string_lossy().into_owned();

    let refused = deployment.run(&["quality", "check", "--root", &root]);
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&refused.stdout),
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(
        !refused.status.success(),
        "a TypeScript product without typecheck passed: {said}"
    );
    assert!(
        said.contains("declares no typecheck script")
            && said.contains("\"typecheck\": \"tsc --noEmit\""),
        "the refusal does not name the script to declare: {said}"
    );

    let mut package: Value =
        serde_json::from_slice(&fs::read(checkout.join("package.json")).unwrap()).unwrap();
    package["scripts"]["typecheck"] = json!("tsc --noEmit");
    package["dependencies"] = json!({"unheld-package": "file:./unheld"});
    fs::write(
        checkout.join("package.json"),
        serde_json::to_vec_pretty(&package).unwrap(),
    )
    .unwrap();
    deployment.git(&checkout, &["add", "package.json"]);
    deployment.git(
        &checkout,
        &[
            "commit",
            "--quiet",
            "-m",
            "declare an unlocked local dependency",
        ],
    );
    deployment.git(&checkout, &["push", "--quiet", "origin", "main"]);

    let install_failed = deployment.run(&["quality", "check", "--root", &root]);
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&install_failed.stdout),
        String::from_utf8_lossy(&install_failed.stderr)
    );
    assert!(
        !install_failed.status.success(),
        "an unlocked dependency passed npm ci: {said}"
    );
    assert!(
        said.contains("npm ci")
            && said.contains("web gate: inspect the failed command and output")
            && !said.contains("`stado quality format` writes what it reads"),
        "the failed web install was sent to a formatter instead of its dependency: {said}"
    );

    fs::write(
        checkout.join("index.ts"),
        "export const site: string = null;\n",
    )
    .unwrap();
    deployment.write_package(&checkout, Some("tsc --noEmit"));
    deployment.git(
        &checkout,
        &["add", "index.ts", "package.json", "package-lock.json"],
    );
    deployment.git(
        &checkout,
        &["commit", "--quiet", "-m", "declare a failing typecheck"],
    );
    deployment.git(&checkout, &["push", "--quiet", "origin", "main"]);

    let failed = deployment.run(&["quality", "check", "--root", &root]);
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&failed.stdout),
        String::from_utf8_lossy(&failed.stderr)
    );
    assert!(
        !failed.status.success(),
        "an invalid source passed typecheck: {said}"
    );
    assert!(
        said.contains("index.ts")
            && said.contains("TS2322")
            && said.contains("web gate: inspect the failed command and output")
            && !said.contains("`stado quality format` writes what it reads"),
    );

    fs::write(
        checkout.join("index.ts"),
        "export const site: string = \"typecheck\";\n",
    )
    .unwrap();
    deployment.git(&checkout, &["add", "index.ts"]);
    deployment.git(
        &checkout,
        &["commit", "--quiet", "-m", "repair the typecheck source"],
    );
    deployment.git(&checkout, &["push", "--quiet", "origin", "main"]);

    let passed = deployment.run(&["quality", "check", "--root", &root]);
    let said = String::from_utf8_lossy(&passed.stdout).into_owned();
    assert!(
        passed.status.success(),
        "the product declaring typecheck was refused: {said}{}",
        String::from_utf8_lossy(&passed.stderr)
    );
    assert!(
        said.contains(&format!("stado web quality: {PRODUCT} passed typecheck")),
        "the gate did not run the declared typecheck: {said}"
    );

    deployment.finish(&checkout);
}
