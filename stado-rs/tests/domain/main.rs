//! Which launchd domain a unit belongs to, read off this machine.
//!
//! The area that used to cover this drove a scripted `launchctl` behind a
//! scripted `ssh`, so it never once observed a loaded unit; it was deleted
//! rather than repaired. This one loads a unit for real — the test writes its
//! own plist and bootstraps it into this user's own `gui/<uid>` domain under a
//! label carrying this process's id — reads it through the product, and
//! compares every claim with what `launchctl` itself reports. Then the product
//! boots it out and the case proves it is gone.
//!
//! The system domain needs a privilege this test does not have, and that shows
//! up in the reports as a named read failure rather than as a claim. That is
//! the second subject here: what a command may say about a unit it could not
//! read.
//!
//! Every sentence asserted below was copied from a live run on 2026-09-08.

mod unit;

use std::path::Path;
use std::process::{Command, Output};

use serde_json::{json, Value};

use unit::{uid, OwnedUnit};

const TARGET: &str = "domain-observation-host";

fn platform() -> &'static str {
    if std::env::consts::OS == "macos" {
        "darwin-arm64"
    } else {
        "linux-amd64"
    }
}

fn hostname() -> String {
    let output = Command::new("/bin/hostname")
        .output()
        .expect("the operating system reports its host name");
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .to_lowercase()
}

fn storage() -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("an isolated storage root");
    let registry = json!({
        "schema_version": 2,
        "targets": [{
            "name": TARGET,
            "kind": "local",
            "ssh": null,
            "release_platform": platform(),
            "hostnames": [hostname()],
            "services": [],
        }],
        "coordinators": [],
    });
    std::fs::write(
        directory.path().join("registry.json"),
        serde_json::to_vec_pretty(&registry).expect("registry serialises"),
    )
    .expect("seed the isolated registry");
    directory
}

fn stado(storage: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(args)
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", storage)
        .env("STADO_CONFIG", storage.join("no-such-config.json"))
        .env_remove("COMPUTE_API_KEY")
        .env_remove("COMPUTE_API_URL")
        .env_remove("WC_PROFILES_DIR")
        .output()
        .expect("the built stado binary runs")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn report(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout was not one JSON report: {error}\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&output.stdout),
            stderr(output)
        )
    })
}

#[test]
fn a_loaded_unit_is_reported_with_the_domain_and_pid_launchd_gives_it() {
    let storage = storage();
    let mut owned = OwnedUnit::declared(storage.path());
    owned.bootstrap();

    let output = stado(
        storage.path(),
        &[
            "service",
            "label-print",
            "--host",
            TARGET,
            &owned.label,
            "--json",
        ],
    );
    let report = report(&output);

    assert_eq!(report["label"], owned.label.as_str());
    assert_eq!(report["loaded"], json!(true));
    assert_eq!(report["read_status"], "loaded");
    assert_eq!(
        report["domain"],
        format!("gui/{}", uid()).as_str(),
        "the unit was bootstrapped into this user's domain"
    );
    assert_eq!(
        report["pid"].as_str().map(str::to_string),
        owned.printed_field("pid"),
        "the reported pid is the one launchd printed"
    );
    // macOS hands back the canonical path, so the temp directory the test
    // wrote into arrives with its /private prefix.
    let declared = owned
        .plist
        .canonicalize()
        .expect("the plist this test wrote is on disk");
    assert_eq!(
        report["path"].as_str().map(Path::new),
        Some(declared.as_path()),
        "the report names the plist this test wrote"
    );
    assert_eq!(report["state"], "active");
}

#[test]
fn the_product_boots_out_the_unit_it_was_asked_about() {
    let storage = storage();
    let mut owned = OwnedUnit::declared(storage.path());
    owned.bootstrap();
    assert!(owned.is_loaded(), "the unit is loaded before it is removed");

    let output = stado(
        storage.path(),
        &[
            "service",
            "bootout",
            "--host",
            TARGET,
            &owned.label,
            "--json",
        ],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    let report = report(&output);
    assert_eq!(report["state"], "booted_out");
    assert_eq!(report["detail"], owned.service_target().as_str());

    owned.forget();
    assert!(
        !owned.is_loaded(),
        "launchd still holds the unit the product reported as booted out"
    );
}

#[test]
fn a_label_nobody_loaded_is_reported_unloaded_and_not_claimed() {
    let storage = storage();
    let label = format!("com.wisent.test.stado-never-loaded-{}", std::process::id());

    let output = stado(
        storage.path(),
        &["service", "label-print", "--host", TARGET, &label, "--json"],
    );
    let report = report(&output);

    assert_eq!(report["loaded"], json!(false));
    assert_eq!(report["domain"], Value::Null, "no domain is invented");
    assert_eq!(report["pid"], Value::Null, "no pid is invented");
    assert_eq!(report["state"], Value::Null);
}

#[test]
fn a_domain_this_process_cannot_read_is_named_rather_than_answered() {
    let storage = storage();
    let mut owned = OwnedUnit::declared(storage.path());
    owned.bootstrap();

    let output = stado(
        storage.path(),
        &[
            "service",
            "label-print",
            "--host",
            TARGET,
            &owned.label,
            "--json",
        ],
    );
    let report = report(&output);

    // The system domain needs a privilege this test does not have. The reader
    // has to say which domain it could not read and why, instead of folding
    // that silence into the answer about the user domain.
    let failures = report["read_failures"]
        .as_array()
        .expect("the reader lists the domains it could not read");
    let system = failures
        .iter()
        .find(|failure| failure["domain"] == "system")
        .expect("the system domain read is accounted for");
    assert!(
        system["detail"]
            .as_str()
            .is_some_and(|detail| !detail.trim().is_empty()),
        "the failure carries the reason the host gave: {system}"
    );
    assert!(
        system["exit_code"].as_i64().is_some_and(|code| code != 0),
        "a failed read carries the exit status: {system}"
    );
    assert_eq!(
        report["loaded"],
        json!(true),
        "the user domain still answered"
    );
}

#[test]
fn a_host_outside_the_registry_is_refused() {
    let storage = storage();
    let output = stado(
        storage.path(),
        &[
            "service",
            "label-print",
            "--host",
            "nowhere-host",
            "com.wisent.test.stado-absent",
            "--json",
        ],
    );

    assert_ne!(
        output.status.code(),
        Some(0),
        "an unknown host cannot be answered: {}",
        stderr(&output)
    );
    assert!(
        stderr(&output).contains("nowhere-host"),
        "{}",
        stderr(&output)
    );
}
