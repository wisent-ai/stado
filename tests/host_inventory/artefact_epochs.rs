//! `stado host inventory TARGET --json` reads a service artefact's age and
//! its installed program's with the remote programs' one modification-time
//! reader, in this platform's own `stat` dialect, so the host's answer parses
//! and both epochs are the files' modification times. A time that cannot be
//! read (the file removed between finding and reading it, or `stat` broken)
//! carries `stat`'s answer instead; no case here can make one, since the
//! inventory reads only files it has just found.
//!
//! One isolated deployment (`config init` seeds a registry naming this
//! machine, with HOME the run directory), compiled into the scheduler test
//! binary so it runs on every platform the release gates run; the inventory
//! program reaches this machine by the host channel's local route.

use std::fs::{self, File};
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::Path;
use std::time::{Duration, SystemTime};

use serde_json::Value;

use crate::deployment::Deployment;

/// The artefact's modification time and the installed copy's, a day apart
/// so neither can be mistaken for the other.
const ARTEFACT_EPOCH: u64 = 1_700_000_000;
const INSTALLED_EPOCH: u64 = 1_700_086_400;

/// An executable file at `path` whose modification time is `epoch`.
fn program_at(path: &Path, epoch: u64) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, b"#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    let written = SystemTime::UNIX_EPOCH + Duration::from_secs(epoch);
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(written)
        .unwrap();
}

#[test]
fn a_service_artefact_and_its_installed_program_are_dated_by_their_files() {
    let mut deployment = Deployment::start("inventory-artefact-epochs");
    let host = deployment
        .cli(&["registry", "self", "--name-only"])
        .trim()
        .to_string();
    let service = deployment.root.join(".stado/services/inventory-journey");
    let release = service.join("sha256-journey");
    program_at(&release.join("platform/journey-tool"), ARTEFACT_EPOCH);
    symlink(&release, service.join("current")).unwrap();
    let installed = deployment.root.join(".stado/bin/journey-tool");
    program_at(&installed, INSTALLED_EPOCH);
    let (_, stdout, stderr) = deployment.run(&["host", "inventory", &host, "--json"], "");
    let report: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|error| panic!("the inventory is not JSON ({error}): {stdout}{stderr}"));
    let rows = report["service_artifacts"]
        .as_array()
        .unwrap_or_else(|| panic!("the inventory lists no service artefacts: {report}"));
    let row = rows
        .iter()
        .find(|row| row["label"] == "inventory-journey")
        .unwrap_or_else(|| panic!("the seeded service is not listed: {report}"));
    assert_eq!(row["program"], "journey-tool", "{row}");
    assert_eq!(row["artefact_epoch"], ARTEFACT_EPOCH.to_string(), "{row}");
    assert_eq!(row["installed_epoch"], INSTALLED_EPOCH.to_string(), "{row}");
    assert_eq!(row["artefact_epoch_error"], "", "{row}");
    assert_eq!(row["installed_epoch_error"], "", "{row}");
    deployment.pass();
}
