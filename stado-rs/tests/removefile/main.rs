//! `stado host remove-file` against the local storage backend.
//!
//! The target is THIS machine, so the command runs its fixed script through
//! the local branch of the host channel — the same `/bin/bash -s` the ssh
//! branch feeds, no ssh involved. HOME is pointed at a tempdir, so the
//! managed areas the guard allows are disposable. Behaviour was probed by
//! hand against the real command before these sentences were written.

use std::path::Path;
use std::process::{Command, Output};

fn stado(storage: &Path, home: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_stado"));
    cmd.args(args)
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", storage)
        .env("STADO_CONFIG", storage.join("no-such-config.json"))
        .env("HOME", home)
        .env_remove("COMPUTE_API_KEY")
        .env_remove("COMPUTE_API_URL");
    cmd.output().expect("stado binary runs")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// A temp storage root whose registry declares this machine as a target, so
/// `canonical_target` resolves it and the host channel takes its local branch.
fn storage_with_this_host(home: &Path) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let hostname = String::from_utf8(Command::new("hostname").output().unwrap().stdout)
        .unwrap()
        .trim()
        .to_string();
    let hostname = hostname.split('.').next().unwrap_or(&hostname).to_string();
    let document = format!(
        r#"{{"schema_version": 2, "targets": [{{
            "name": "this-mac",
            "kind": "local",
            "ssh": null,
            "release_platform": "darwin-arm64",
            "hostnames": ["{hostname}"],
            "slots": 1
        }}], "coordinators": []}}"#
    );
    std::fs::write(dir.path().join("registry.json"), document).unwrap();
    let _ = home;
    dir
}

fn home_with_plist(name: &str) -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    let agents = home.path().join("Library/LaunchAgents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(agents.join(name), b"<plist/>").unwrap();
    home
}

#[test]
fn an_allowed_plist_is_removed() {
    let home = home_with_plist("com.wisent.compute.service.stado-agent-mini.plist");
    let storage = storage_with_this_host(home.path());
    let path = home
        .path()
        .join("Library/LaunchAgents/com.wisent.compute.service.stado-agent-mini.plist");
    let out = stado(
        storage.path(),
        home.path(),
        &["host", "remove-file", "this-mac", path.to_str().unwrap()],
    );
    assert!(out.status.success(), "removal failed: {}", stderr(&out));
    assert!(stdout(&out).contains("removed"), "{}", stdout(&out));
    assert!(!path.exists(), "the file must be gone");
}

#[test]
fn an_absent_file_reports_absent_without_failing() {
    let home = tempfile::tempdir().unwrap();
    let storage = storage_with_this_host(home.path());
    let path = home.path().join("Library/LaunchAgents/none.plist");
    let out = stado(
        storage.path(),
        home.path(),
        &["host", "remove-file", "this-mac", path.to_str().unwrap()],
    );
    assert!(
        out.status.success(),
        "absence must not fail: {}",
        stderr(&out)
    );
    assert!(stdout(&out).contains("absent"), "{}", stdout(&out));
}

#[test]
fn a_symlink_is_refused_and_left_alone() {
    let home = home_with_plist("real.plist");
    let agents = home.path().join("Library/LaunchAgents");
    std::os::unix::fs::symlink(agents.join("real.plist"), agents.join("link.plist")).unwrap();
    let storage = storage_with_this_host(home.path());
    let path = agents.join("link.plist");
    let out = stado(
        storage.path(),
        home.path(),
        &["host", "remove-file", "this-mac", path.to_str().unwrap()],
    );
    assert!(!out.status.success(), "a symlink must not pass");
    assert!(
        stderr(&out).contains("a symlink points outside the managed area"),
        "the refusal names the reason: {}",
        stderr(&out)
    );
    assert!(agents.join("real.plist").exists(), "the target stays");
}

#[test]
fn a_directory_is_refused_and_left_alone() {
    let home = home_with_plist("real.plist");
    let storage = storage_with_this_host(home.path());
    let path = home.path().join("Library/LaunchAgents");
    let out = stado(
        storage.path(),
        home.path(),
        &["host", "remove-file", "this-mac", path.to_str().unwrap()],
    );
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("a directory is not removed by a single-file command"),
        "got: {}",
        stderr(&out)
    );
    assert!(path.exists());
}

#[test]
fn a_system_path_is_refused_with_the_privileged_command_named() {
    let home = tempfile::tempdir().unwrap();
    let storage = storage_with_this_host(home.path());
    let out = stado(
        storage.path(),
        home.path(),
        &[
            "host",
            "remove-file",
            "this-mac",
            "/Library/LaunchDaemons/com.wisent.example.plist",
        ],
    );
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains(
            "outside the managed home areas; remove it on the host with: sudo rm -- /Library/LaunchDaemons/com.wisent.example.plist"
        ),
        "the refusal names the privileged command: {}",
        stderr(&out)
    );
}

#[test]
fn a_relative_or_dotdot_path_never_reaches_the_host() {
    let home = tempfile::tempdir().unwrap();
    let storage = storage_with_this_host(home.path());
    for bad in ["Library/LaunchAgents/x.plist", "/tmp/../etc/passwd"] {
        let out = stado(
            storage.path(),
            home.path(),
            &["host", "remove-file", "this-mac", bad],
        );
        assert_eq!(
            out.status.code(),
            Some(2),
            "path {bad} must be a usage error"
        );
        assert!(
            stderr(&out).contains("must be absolute, contain no '..', and carry no NUL"),
            "got: {}",
            stderr(&out)
        );
    }
}
