//! `release host-state` refreshes persisted software observations even when
//! the actual executable in an isolated home has no release attestation.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::fixture::{report, stderr, Fixture, TARGET};

/// The built product under an isolated home, without a staged attestation.
struct Home {
    root: tempfile::TempDir,
}

impl Home {
    fn new() -> Self {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/host-release-test-runs");
        std::fs::create_dir_all(&directory).expect("create repository test directory");
        let root = tempfile::tempdir_in(directory).expect("an isolated home directory");
        let bin = root.path().join(".stado").join("bin");
        std::fs::create_dir_all(&bin).expect("create the isolated bin directory");
        let source = Path::new(env!("CARGO_BIN_EXE_stado"));
        let destination = bin.join("stado");
        if std::fs::hard_link(source, &destination).is_err() {
            std::fs::copy(source, &destination).expect("copy the managed binary into the home");
        }
        Self { root }
    }

    fn path(&self) -> &Path {
        self.root.path()
    }

    fn binary(&self) -> PathBuf {
        self.path().join(".stado").join("bin").join("stado")
    }

    fn observations(&self) -> Vec<Value> {
        let text = std::fs::read_to_string(self.path().join(".stado").join("observations.json"))
            .expect("host-state leaves an observation store in the home it read");
        serde_json::from_str(&text).expect("the observation store stays valid JSON")
    }
}

#[test]
fn host_state_writes_the_software_report_release_status_reads() {
    let identity = Command::new(env!("CARGO_BIN_EXE_stado"))
        .arg("--version")
        .output()
        .expect("the built product reports its version");
    assert!(identity.status.success(), "{}", stderr(&identity));
    let identity = String::from_utf8(identity.stdout).expect("the version is UTF-8");
    let version = identity
        .split_whitespace()
        .nth(1)
        .expect("stado --version names its semantic version");
    let fixture = Fixture::new();
    let home = Home::new();
    let declared = fixture.declare(version);
    assert!(declared.status.success(), "{}", stderr(&declared));

    let output = fixture.host_state_in_home(home.path(), &[]);
    let report = report(&output);
    // The home holds no staged copy of the binary, so the drift gate refuses
    // the bytes as unattested and the command exits non-zero. The report is
    // written on that visit all the same: a refresh that only happened when
    // the verdict was clean would leave `release status` stale on exactly
    // the hosts it most needs to describe.
    assert!(!output.status.success(), "{}", stderr(&output));
    assert_eq!(report["binaries"][0]["verdict"], "unattested");

    // What the command says it put on file.
    let software = &report["software"];
    assert_eq!(software["host"], TARGET);
    assert_eq!(software["state"], "observed");
    assert_eq!(software["observed"], "just now");
    let programs = software["programs"]
        .as_array()
        .expect("the software block lists the programs it rowed");
    let stado = programs
        .iter()
        .find(|program| program["name"] == "stado")
        .expect("the managed binary in the home's bin directory is rowed");
    assert_eq!(stado["path"], home.binary().to_string_lossy().as_ref());
    assert_eq!(
        stado["version"], version,
        "the row carries the version the binary prints"
    );
    assert_eq!(
        stado["provenance"], "unmanaged",
        "a home with no staged release has nothing to attest the bytes against"
    );

    // What is actually on file, which is what `release status` judges next.
    let rows = home.observations();
    let roster = rows
        .iter()
        .find(|row| row["fact"] == format!("software-report:{TARGET}"))
        .expect("the report's own roster row is on file");
    assert_eq!(roster["vantage"], TARGET);
    assert_eq!(roster["state"], "observed");
    let row = rows
        .iter()
        .find(|row| row["fact"] == format!("software:stado@{TARGET}"))
        .expect("the managed binary's row is on file");
    let detail = row["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains(&format!("version={version} ")),
        "the stored row carries the version: {detail}"
    );
    assert!(
        detail.ends_with(&format!("path={}", home.binary().display())),
        "the stored row names the file it read: {detail}"
    );
}
