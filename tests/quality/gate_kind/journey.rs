//! Exercise gate selection with a real Rust source, compiler and executable
//! assertion through `stado quality check`: only `test` gates run, and the
//! compile is part of the test that needs it.

use super::{Deployment, MANIFEST};
use serde_json::json;
use std::{fs, path::Path};

#[path = "handoff.rs"]
mod handoff;

/// The test gate that compiles the source's tests and runs them: the
/// compile is part of the test, as every test gate's is.
fn compiled_test() -> serde_json::Value {
    json!({"name": "journey", "kind": "test", "argv": ["sh", "-c", "rustc --test src/lib.rs -o checks && ./checks"]})
}

#[test]
fn the_check_runs_only_test_gates() {
    let mut deployment = Deployment::start();
    let initialised = deployment.run(&["config", "init"]);
    assert!(
        initialised.status.success(),
        "config init: {}",
        String::from_utf8_lossy(&initialised.stderr)
    );

    // A format gate and a lint gate that would refuse are declared beside
    // the test: neither runs, so the passing test passes the check.
    let passing = deployment.checkout(
        "passing",
        "pub fn answer() -> bool {\n    true\n}\n\n#[test]\nfn answers_true() {\n    assert!(answer());\n}\n",
        json!([
            {"name": "style", "kind": "format", "argv": ["sh", "-c", "exit 4"]},
            {"name": "verdict", "kind": "lint", "argv": ["sh", "-c", "exit 3"]},
            compiled_test()
        ]),
    );
    let passed = deployment.run(&["quality", "check", "--root", &passing]);
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&passed.stdout),
        String::from_utf8_lossy(&passed.stderr)
    );
    assert!(
        passed.status.success(),
        "a format or lint gate ran, or the passing test was refused: {said}"
    );
    assert!(
        said.contains("test result: ok"),
        "the test gate's own assertion was not observed: {said}"
    );

    // Source the compiler refuses fails the test that compiles it, and the
    // refusal names that test and the compiler's own words.
    let not_compiling = deployment.checkout(
        "not-compiling",
        "pub fn answer() -> bool {\n    missing_answer\n}\n\n#[test]\nfn answers_true() {\n    assert!(answer());\n}\n",
        json!([compiled_test()]),
    );
    let refused = deployment.run(&["quality", "check", "--root", &not_compiling]);
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&refused.stdout),
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(
        !refused.status.success(),
        "the check passed source whose compiler refuses: {said}"
    );
    assert!(
        said.contains("gate \"journey\" (test)") && said.contains("missing_answer"),
        "the compiler refusal was hidden or not named as the test's: {said}"
    );

    // A compiled test whose assertion fails is refused.
    let failing_test = deployment.checkout(
        "failing-test",
        "pub fn answer() -> bool {\n    false\n}\n\n#[test]\nfn answers_true() {\n    assert!(answer());\n}\n",
        json!([compiled_test()]),
    );
    let refused = deployment.run(&["quality", "check", "--root", &failing_test]);
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&refused.stdout),
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(
        !refused.status.success(),
        "a failing executable test passed: {said}"
    );
    assert!(
        said.contains("gate \"journey\" (test)") && said.contains("answers_true"),
        "the test failure was hidden: {said}"
    );

    // A present but incomplete declaration is not an absent product.
    let no_kind = deployment.checkout(
        "no-kind",
        "pub fn answer() -> bool {\n    true\n}\n",
        json!([{"name": "fmt", "argv": ["rustfmt", "--check", "src/lib.rs"]}]),
    );
    let refused = deployment.run(&["quality", "check", "--root", &no_kind]);
    let said = String::from_utf8_lossy(&refused.stderr).into_owned();
    assert!(!refused.status.success(), "a gate without a kind passed");
    assert!(
        said.contains("kind"),
        "the refusal does not name the missing field: {said}"
    );

    let undeclared = deployment.root.join("undeclared");
    fs::create_dir_all(&undeclared).unwrap();
    deployment.git(&undeclared, &["init", "--quiet", "--initial-branch=main"]);
    fs::write(undeclared.join("README"), "No release recipe\n").unwrap();
    deployment.git(&undeclared, &["add", "README"]);
    deployment.git(&undeclared, &["commit", "--quiet", "-m", "add source"]);
    let undeclared = undeclared.to_str().unwrap();
    let revision = super::git_output(
        Path::new(undeclared),
        &["rev-parse", "HEAD"],
        &mut deployment.report,
        &deployment.root.join("report.json"),
    );
    let optional = deployment.run(&[
        "quality",
        "check",
        "--root",
        undeclared,
        "--if-declared",
        "--json",
    ]);
    assert!(
        optional.status.success(),
        "optional check refused a product without a declaration: {}",
        String::from_utf8_lossy(&optional.stderr)
    );
    let checked: serde_json::Value = serde_json::from_slice(&optional.stdout).unwrap();
    assert_eq!(checked["status"], "passed", "{checked}");
    assert_eq!(checked["revisions"], json!([revision.trim()]), "{checked}");
    let strict = deployment.run(&["quality", "check", "--root", undeclared, "--json"]);
    assert!(
        !strict.status.success(),
        "explicit check accepted an undeclared product"
    );
    let refusal: serde_json::Value = serde_json::from_slice(&strict.stdout).unwrap();
    assert_eq!(refusal["status"], "error", "{refusal}");
    assert_eq!(refusal["error_code"], "refused", "{refusal}");
    assert!(
        refusal["message"].as_str().unwrap().contains(MANIFEST),
        "the refusal did not name the missing declaration: {refusal}"
    );

    #[cfg(unix)]
    {
        let checkout = Path::new(undeclared);
        std::os::unix::fs::symlink("missing-release.json", checkout.join(MANIFEST)).unwrap();
        deployment.git(checkout, &["add", MANIFEST]);
        deployment.git(
            checkout,
            &["commit", "--quiet", "-m", "declare missing recipe link"],
        );
        let broken = deployment.run(&[
            "quality",
            "check",
            "--root",
            undeclared,
            "--if-declared",
            "--json",
        ]);
        assert!(
            !broken.status.success(),
            "an unreadable manifest was skipped"
        );
        let refusal: serde_json::Value = serde_json::from_slice(&broken.stdout).unwrap();
        assert_eq!(refusal["status"], "error", "{refusal}");
        assert_eq!(refusal["error_code"], "not_found", "{refusal}");
        assert!(
            refusal["message"].as_str().unwrap().contains(MANIFEST),
            "the refusal did not identify the unreadable declaration: {refusal}"
        );
    }

    deployment.report["outcome"] = json!("passed");
    deployment.save();
    eprintln!("report: {}", deployment.root.join("report.json").display());
}
