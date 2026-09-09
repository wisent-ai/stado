//! The row `doctor::object_auth_verdict` builds, read where an operator reads
//! it.
//!
//! The four verifier results the function judges are produced by
//! `crate::skarbiec::validate_*_verifier`, and the only command that publishes
//! its judgement is `stado doctor`. This case drives the deployment preflight
//! against an isolated environment that declares no gateway boundary at all,
//! which is the arm of the judgement a test can reach: every verifier answers
//! with a configuration verdict, so the row must FAIL, must say why in the
//! sentence the product owns, and must name each of the four boundaries
//! separately rather than letting the first one it reads stand for all of them.
//!
//! The other arm — a vault that answered `5xx`, which must read UNMEASURED
//! rather than as an authorization failure — is not reachable from here: each
//! verifier validates its own configuration before it ever contacts the vault,
//! and that validation requires every active namespace, publisher, machine
//! client and deployed service the fleet declares. A fixture that reproduced
//! those four lists would be pinned to the fleet's deployment rather than to
//! this judgement. The PR body names it.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

use crate::fixture::said;

/// The row's identity and the sentence it reports a measured configuration
/// verdict with, copied from a live `stado doctor --deployment-preflight
/// --json` run against this fixture.
const ROW_ID: &str = "object-auth";
const ROW_TITLE: &str = "Object, release, machine, and service gateway auth";
const FAILS_CLOSED: &str = "authorization fails closed because mapping, verifier grant, or mapped \
                            token validation failed";
const NOT_MEASURED: &str = "not measured:";
const FAIL: &str = "fail";

/// The four boundaries the row judges, each named in the detail with its own
/// problem.
const VERIFIERS: [&str; 4] = [
    "product verifier",
    "release verifier",
    "machine verifier",
    "service verifier",
];

/// `stado doctor` exits non-zero when any check FAILs.
const DOCTOR_FAILED: i32 = 1;

/// An unconfigured gateway boundary is a measured verdict: the row FAILs, says
/// so in its own sentence, and names all four verifiers.
#[test]
fn the_object_auth_row_reports_a_configuration_verdict_as_a_failure() {
    let dir = tempfile::Builder::new()
        .prefix("stado-object-auth-doctor-")
        .tempdir()
        .expect("create the isolated preflight environment");
    let root = dir.path().to_path_buf();
    let home = root.join("home");
    let storage = root.join("storage");
    for directory in [&home, &storage] {
        fs::create_dir_all(directory).expect("create the isolated directory");
    }

    let output = preflight(&root, &home, &storage);
    assert_eq!(
        output.status.code(),
        Some(DOCTOR_FAILED),
        "the preflight exited {:?}\nstderr:\n{}",
        output.status.code(),
        said(&output.stderr)
    );

    let report: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "the preflight did not print one JSON report: {error}\n{}",
            said(&output.stdout)
        )
    });
    let row = report["checks"]
        .as_array()
        .expect("the preflight reports its checks")
        .iter()
        .find(|check| check["id"] == ROW_ID)
        .unwrap_or_else(|| panic!("the preflight has no {ROW_ID} row: {report}"));

    assert_eq!(row["title"], ROW_TITLE);
    assert_eq!(
        row["status"], FAIL,
        "a boundary that declares nothing is a measured verdict: {row}"
    );
    let detail = row["detail"]
        .as_str()
        .unwrap_or_else(|| panic!("the row carries a detail: {row}"));
    assert!(
        detail.starts_with(FAILS_CLOSED),
        "the row did not report the verdict in the product's own sentence: {detail}"
    );
    assert!(
        !detail.contains(NOT_MEASURED),
        "a configuration verdict was reported as unmeasured: {detail}"
    );
    for verifier in VERIFIERS {
        assert!(
            detail.contains(verifier),
            "the row does not name {verifier}, so one boundary's problem stands \
             for all four: {detail}"
        );
    }
    assert!(
        row["remedy"]
            .as_str()
            .expect("the row carries a remedy")
            .contains("install their distinct owner-only verifier grants"),
        "the row does not say what to install: {row}"
    );
}

fn preflight(root: &PathBuf, home: &PathBuf, storage: &PathBuf) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(["doctor", "--deployment-preflight", "--json"])
        .env_clear()
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("TMPDIR", root)
        .env("STADO_CONFIG", root.join("no-such-config.json"))
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", storage)
        .env("WC_PROVIDERS", "local")
        .env("NO_COLOR", "1")
        .output()
        .expect("the built stado binary did not start")
}
