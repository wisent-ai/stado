//! Real CLI journeys for target-scoped run-input delivery.
//!
//! The registry target is the machine executing the test, so the production
//! host channel takes its local branch while still resolving a canonical Stado
//! target. `rsync`, shell guards, atomic replacement, mode preservation and
//! symlink handling are the real system programs; HOME and storage are isolated.

use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

const TARGET: &str = "delivery-current-host";
const RUN: &str = "123e4567-e89b-12d3-a456-426614174000";

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

struct Fleet {
    root: tempfile::TempDir,
    home: PathBuf,
    storage: PathBuf,
    source: PathBuf,
}

impl Fleet {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let storage = root.path().join("storage");
        let source = root.path().join("source");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&storage).unwrap();
        fs::create_dir_all(&source).unwrap();
        let hostname = String::from_utf8(Command::new("hostname").output().unwrap().stdout)
            .unwrap()
            .trim()
            .to_ascii_lowercase();
        fs::write(
            storage.join("registry.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema_version": 2,
                "targets": [{
                    "name": TARGET,
                    "kind": "local",
                    "ssh": null,
                    "release_platform": "darwin-arm64",
                    "hostnames": [hostname],
                    "services": [],
                }],
                "coordinators": [],
            }))
            .unwrap(),
        )
        .unwrap();
        Self {
            root,
            home,
            storage,
            source,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("STADO_CONFIG", self.storage.join("no-such-config.json"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &self.storage)
            .env("WC_PROVIDERS", "local")
            .env("NO_COLOR", "1");
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().expect("stado runs")
    }

    fn deliver_with_stdin(&self, destination: &str, file_list: &[u8]) -> Output {
        let mut child = self
            .command()
            .args([
                "host",
                "deliver",
                TARGET,
                self.source.to_str().unwrap(),
                destination,
                "--files-from",
                "-",
                "--json",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("stado starts");
        std::io::Write::write_all(child.stdin.as_mut().unwrap(), file_list).unwrap();
        child.wait_with_output().unwrap()
    }

    fn destination(&self, name: &str) -> PathBuf {
        self.home
            .join(".stado/work/runs")
            .join(RUN)
            .join(name)
    }

    fn valid_destination(name: &str) -> String {
        format!(".stado/work/runs/{RUN}/{name}")
    }
}

#[test]
fn a_selected_uncommitted_tree_is_replaced_with_modes_and_symlinks_preserved() {
    let fleet = Fleet::new();
    fs::create_dir_all(fleet.source.join("nested")).unwrap();
    fs::write(fleet.source.join("run.sh"), b"#!/bin/sh\necho first\n").unwrap();
    fs::set_permissions(fleet.source.join("run.sh"), fs::Permissions::from_mode(0o751)).unwrap();
    fs::write(fleet.source.join("nested/data.txt"), b"selected\n").unwrap();
    fs::write(fleet.source.join("ignored.txt"), b"not selected\n").unwrap();
    symlink("nested/data.txt", fleet.source.join("current")).unwrap();

    let destination = Fleet::valid_destination("probierz");
    let first = fleet.deliver_with_stdin(
        &destination,
        b"run.sh\0nested/data.txt\0current\0",
    );
    assert!(first.status.success(), "{}{}", stdout(&first), stderr(&first));
    let receipt: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(receipt["status"], "delivered");
    assert_eq!(receipt["selection"], "nul-file-list");
    let delivered = fleet.destination("probierz");
    assert_eq!(fs::read(delivered.join("nested/data.txt")).unwrap(), b"selected\n");
    assert!(!delivered.join("ignored.txt").exists());
    assert_eq!(fs::symlink_metadata(delivered.join("current")).unwrap().file_type().is_symlink(), true);
    assert_eq!(fs::metadata(delivered.join("run.sh")).unwrap().permissions().mode() & 0o777, 0o751);

    fs::write(fleet.source.join("run.sh"), b"#!/bin/sh\necho second\n").unwrap();
    let second = fleet.deliver_with_stdin(&destination, b"run.sh\0current\0nested/data.txt\0");
    assert!(second.status.success(), "{}{}", stdout(&second), stderr(&second));
    assert_eq!(fs::read(delivered.join("run.sh")).unwrap(), b"#!/bin/sh\necho second\n");
    assert!(!delivered.join(".probierz.stado-previous").exists());
}

#[test]
fn an_application_bundle_is_delivered_as_a_complete_mode_preserving_tree() {
    let fleet = Fleet::new();
    let bundle = fleet.source.join("Byk Preview.app");
    let executable = bundle.join("Contents/MacOS/Byk");
    fs::create_dir_all(executable.parent().unwrap()).unwrap();
    fs::write(&executable, b"#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    let destination = Fleet::valid_destination("Byk.app");
    let output = fleet.run(&[
        "host",
        "deliver",
        TARGET,
        bundle.to_str().unwrap(),
        &destination,
        "--json",
    ]);
    assert!(output.status.success(), "{}{}", stdout(&output), stderr(&output));
    let delivered = fleet.destination("Byk.app/Contents/MacOS/Byk");
    assert_eq!(fs::metadata(&delivered).unwrap().permissions().mode() & 0o777, 0o755);
}

#[test]
fn outside_destination_is_refused_before_any_transfer() {
    let fleet = Fleet::new();
    fs::write(fleet.source.join("payload"), b"never copied").unwrap();
    let output = fleet.run(&[
        "host",
        "deliver",
        TARGET,
        fleet.source.join("payload").to_str().unwrap(),
        "/tmp/probierz-delivery-must-not-exist",
    ]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains(
            "destination \"/tmp/probierz-delivery-must-not-exist\" is outside the managed area"
        ),
        "{}",
        stderr(&output)
    );
}

#[test]
fn a_symlink_in_the_destination_path_is_refused_before_transfer() {
    let fleet = Fleet::new();
    fs::write(fleet.source.join("payload"), b"never copied").unwrap();
    let outside = fleet.root.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::create_dir_all(fleet.home.join(".stado")).unwrap();
    symlink(&outside, fleet.home.join(".stado/work")).unwrap();
    let destination = Fleet::valid_destination("payload");
    let output = fleet.run(&[
        "host",
        "deliver",
        TARGET,
        fleet.source.join("payload").to_str().unwrap(),
        &destination,
    ]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("delivery refused before transfer: destination traverses a symlink"),
        "{}",
        stderr(&output)
    );
    assert!(!outside.join("runs").exists(), "the symlink target stayed untouched");
}

#[test]
fn missing_and_unknown_targets_have_exact_refusals() {
    let fleet = Fleet::new();
    fs::write(fleet.source.join("payload"), b"never copied").unwrap();
    let missing = fleet.run(&["host", "deliver"]);
    assert_eq!(missing.status.code(), Some(2));
    assert!(
        stderr(&missing).contains("the following required arguments were not provided"),
        "{}",
        stderr(&missing)
    );
    assert!(stderr(&missing).contains("<TARGET>"), "{}", stderr(&missing));

    let destination = Fleet::valid_destination("payload");
    let unknown = fleet.run(&[
        "host",
        "deliver",
        "not-in-registry",
        fleet.source.join("payload").to_str().unwrap(),
        &destination,
    ]);
    assert!(!unknown.status.success());
    assert!(
        stderr(&unknown).contains("target 'not-in-registry' is not in the canonical registry"),
        "{}",
        stderr(&unknown)
    );
}

#[test]
fn fixed_host_exec_entry_prepares_only_the_managed_run_root() {
    let fleet = Fleet::new();
    let output = fleet.run(&[
        "host",
        "exec",
        TARGET,
        "--",
        "mkdir",
        "-p",
        ".stado/work/runs",
    ]);
    assert!(output.status.success(), "{}{}", stdout(&output), stderr(&output));
    let root = fleet.home.join(".stado/work/runs");
    assert!(root.is_dir());
    assert_eq!(fs::metadata(root).unwrap().permissions().mode() & 0o777, 0o700);
}
