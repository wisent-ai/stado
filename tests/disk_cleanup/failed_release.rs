//! A Stado release recorded as failed on this host is never installed here
//! again: `release local restore` of it is refused naming the record and
//! its cause, and `~/.stado/bin/stado` is left as it was. The record is the
//! one a `stado serve` writes when its own rules refuse the host's
//! configuration and it puts the previous release back.

use std::fs;

use flate2::write::GzEncoder;

use crate::native::Native;

const PLATFORM: &str = "darwin-arm64";
const ARCHIVE: &str = "stado-reader-convergence.tar.gz";

/// A retained release of `version` whose archive holds one `stado`.
fn retain(native: &Native, version: &str) -> std::path::PathBuf {
    let directory = native.home.join(".stado/releases/stado").join(version).join(PLATFORM);
    fs::create_dir_all(&directory).unwrap();
    let program = format!("#!/bin/sh\necho stado {version}\n");
    let mut header = tar::Header::new_gnu();
    header.set_size(program.len() as u64);
    header.set_cksum();
    let mut bundle = tar::Builder::new(GzEncoder::new(Vec::new(), flate2::Compression::default()));
    bundle.append_data(&mut header, "bin/stado", program.as_bytes()).unwrap();
    fs::write(directory.join(ARCHIVE), bundle.into_inner().unwrap().finish().unwrap()).unwrap();
    directory
}

#[test]
fn a_release_recorded_as_failed_is_not_restored() {
    let native = Native::new("failed-release");
    let directory = retain(&native, "0.2.0");
    let cause = "this resolver is declared to serve the store it must read first. Retrying cannot resolve it.";
    let record = directory.join("failed-on-this-host.json");
    fs::write(&record, serde_json::json!({ "version": "0.2.0", "cause": cause }).to_string()).unwrap();
    let listed = native.json(&["release", "local", "list", "--json"]);
    native.observe("list-with-failure", listed.clone());
    let release = listed["releases"]
        .as_array()
        .and_then(|releases| releases.iter().find(|release| release["version"] == "0.2.0"))
        .cloned()
        .expect("the retained release is listed");
    assert_eq!(release["failed"]["cause"], cause, "{listed}");
    assert_eq!(release["installed"], false, "{listed}");


    let output = native.run(&["release", "local", "restore", "--version", "0.2.0"]);
    let said = String::from_utf8_lossy(&output.stderr);
    native.observe("restore-failed", serde_json::json!({ "status": output.status.code(), "stderr": said }));
    assert!(!output.status.success(), "a release recorded as failed was restored: {said}");
    assert!(
        said.contains("Stado 0.2.0 failed on this host and is not installed here again")
            && said.contains(cause)
            && said.contains(&record.display().to_string()),
        "the refusal does not name the failure and its record: {said}"
    );
    assert!(!native.home.join(".stado/bin/stado").exists(), "the failed release was installed anyway");

    fs::remove_file(&record).unwrap();
    let output = native.run(&["release", "local", "restore", "--version", "0.2.0"]);
    let said = String::from_utf8_lossy(&output.stderr);
    native.observe("restore-after-record-removed", serde_json::json!({ "status": output.status.code(), "stderr": said }));
    assert!(
        !said.contains("failed on this host"),
        "a release whose record was removed is still refused as failed: {said}"
    );
}
