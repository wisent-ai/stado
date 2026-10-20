//! Exercise format, lint and test gate selection with a real Rust source,
//! compiler and executable assertion through `stado quality check`.

use super::{Deployment, MANIFEST};
use serde_json::json;
use std::{fs, path::Path};

#[test]
fn the_check_runs_every_gate_the_installation_runs() {
    let mut deployment = Deployment::start();
    let initialised = deployment.run(&["config", "init"]);
    assert!(
        initialised.status.success(),
        "config init: {}",
        String::from_utf8_lossy(&initialised.stderr)
    );

    // Formatted committed source reaches the compiler, which names the
    // missing symbol rather than a formatting failure.
    let lint_fails = deployment.checkout(
        "lint-fails",
        "pub fn answer() -> bool {\n    missing_answer\n}\n\n#[test]\nfn answers_true() {\n    assert!(answer());\n}\n",
        json!([
            {"name": "style", "kind": "format", "argv": ["rustfmt", "--check", "src/lib.rs"]},
            {"name": "compile-tests", "kind": "lint", "argv": ["rustc", "--test", "src/lib.rs", "-o", "checks"]}
        ]),
    );
    let refused = deployment.run(&["quality", "check", "--root", &lint_fails]);
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
        said.contains("stado quality check: rustfmt --check src/lib.rs")
            && said.contains("stado quality check: rustc --test src/lib.rs -o checks"),
        "the check did not run format before the compiler: {said}"
    );
    assert!(
        said.contains("gate \"compile-tests\" (lint)") && said.contains("missing_answer"),
        "the compiler refusal was hidden or sent to the formatter: {said}"
    );

    // A passing compiler gate emits an actual test binary. The test gate
    // executes its assertion, not a command that merely exits zero.
    let lint_and_test = deployment.checkout(
        "lint-and-test",
        "pub fn answer() -> bool {\n    true\n}\n\n#[test]\nfn answers_true() {\n    assert!(answer());\n}\n",
        json!([
            {"name": "compile-tests", "kind": "lint", "argv": ["rustc", "--test", "src/lib.rs", "-o", "checks"]},
            {"name": "journey", "kind": "test", "argv": ["./checks"]}
        ]),
    );
    let passed = deployment.run(&["quality", "check", "--root", &lint_and_test]);
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&passed.stdout),
        String::from_utf8_lossy(&passed.stderr)
    );
    assert!(
        passed.status.success(),
        "a commit whose compiled test passes was refused: {said}"
    );
    assert!(
        said.contains("stado quality check: rustc --test src/lib.rs -o checks")
            && said.contains("stado quality check: ./checks")
            && said.contains("test result: ok"),
        "the compiler and executable assertion were not observed: {said}"
    );

    // Compiling the test executable is not a passing behavioral test.
    let failing_test = deployment.checkout(
        "failing-test",
        "pub fn answer() -> bool {\n    false\n}\n\n#[test]\nfn answers_true() {\n    assert!(answer());\n}\n",
        json!([
            {"name": "compile-tests", "kind": "lint", "argv": ["rustc", "--test", "src/lib.rs", "-o", "checks"]},
            {"name": "journey", "kind": "test", "argv": ["./checks"]}
        ]),
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
        said.contains("stado quality check: ./checks")
            && said.contains("gate \"journey\" (test)")
            && said.contains("answers_true"),
        "the test failure was hidden or attributed to the compiler: {said}"
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
