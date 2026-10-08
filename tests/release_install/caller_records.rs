//! A caller record this account cannot read does not stop a release install.
//! One isolated home: a delivered archive with one member, its digest, and a
//! caller record under `~/.stado/callers` that this account may not read, as
//! a `sudo stado` leaves one (owned by root, mode 600). The real `stado
//! release local install` must install the member, name the record it left
//! out, and leave the record in place. Before, it ended with nothing but
//! `Permission denied (os error 13)` and installed nothing, on every release.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const MEMBER_BODY: &[u8] = b"#!/bin/sh\necho delivered\n";

fn archive(root: &std::path::Path) -> PathBuf {
    let staging = root.join("staging");
    fs::create_dir_all(staging.join("bin")).unwrap();
    fs::write(staging.join("bin/delivered-tool"), MEMBER_BODY).unwrap();
    let file = fs::File::create(root.join("release.tar.gz")).unwrap();
    let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    let mut builder = tar::Builder::new(encoder);
    builder
        .append_path_with_name(staging.join("bin/delivered-tool"), "bin/delivered-tool")
        .unwrap();
    builder.into_inner().unwrap().finish().unwrap();
    root.join("release.tar.gz")
}

#[test]
fn an_unreadable_caller_record_is_named_and_the_release_installs() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join(".build/release-install-caller-records")
        .join(uuid::Uuid::new_v4().to_string());
    let home = root.join("home");
    let callers = home.join(".stado/callers");
    fs::create_dir_all(&callers).unwrap();
    fs::create_dir_all(root.join("tmp")).unwrap();
    let record = callers.join("written-by-root.json");
    fs::write(&record, br#"{"caller":"/usr/bin/sudo"}"#).unwrap();
    fs::set_permissions(&record, fs::Permissions::from_mode(0o000)).unwrap();
    let archive = archive(&root);
    let digest = hex::encode(Sha256::digest(fs::read(&archive).unwrap()));
    let stado = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_stado"))
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", &home)
            .env("STADO_CONFIG", home.join(".stado/config.json"))
            .env("TMPDIR", root.join("tmp"))
            .env("WISENT_RELEASE_ARCHIVE", &archive)
            .env("WISENT_RELEASE_SHA256", &digest)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };
    // A local deployment: its own config and registry, nothing of this machine's.
    let init = stado(&["config", "init"]);
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    let output = stado(&["release", "local", "install", "--member", "bin/delivered-tool"]);
    let report = json!({
        "exit_status": output.status.code(),
        "stdout": String::from_utf8_lossy(&output.stdout),
        "stderr": String::from_utf8_lossy(&output.stderr),
    });
    fs::write(
        root.join("report.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
    // The record goes back to readable so the evidence tree can be removed.
    let installed = fs::read(home.join(".stado/bin/delivered-tool")).ok();
    let still_there = record.exists();
    fs::set_permissions(&record, fs::Permissions::from_mode(0o600)).unwrap();

    let stderr: &Value = &report["stderr"];
    assert!(
        output.status.success(),
        "the install must not stop on the record: {report}"
    );
    assert_eq!(
        installed.as_deref(),
        Some(MEMBER_BODY),
        "the member is installed: {report}"
    );
    assert!(
        stderr
            .as_str()
            .unwrap()
            .contains("cannot be read by this account")
            && stderr.as_str().unwrap().contains("written-by-root.json"),
        "the record left out of the check is named: {report}"
    );
    assert!(
        still_there,
        "a record this account cannot read is not deleted"
    );
    eprintln!(
        "release-install-caller-records evidence: {}",
        root.display()
    );
}
