//! Declared repair, observed on the machine running this test.
//!
//! The area this replaces was deleted in `wisent-ai/stado#566` and the reason
//! was right: it drove the binary against a tempdir, asserted that a
//! declaration parses and that a refusal sentence is exact, and called that
//! evidence for repairing a service. So the host here is this machine,
//! declared in an isolated local registry the way `tests/host_exec` does it,
//! and the read-only report therefore inspects a real host through the real
//! current-host path. The assertions compare what the product reports against
//! what the filesystem and the installed binary really say, so a fabricated or
//! zeroed report fails.
//!
//! The mutating half is deliberately not exercised. Every declared remedy in
//! `data/service-catalog.json` restores a production service — the Stado host
//! program, the object API, a release store, a stable bind — and applying one
//! from a test would cycle a live service on the operator's machine. That is
//! an absent leg stated out loud, not a stub standing in for it.
//!
//! Every sentence asserted below was copied from a live run on 2026-09-08.

mod fixture;

use std::path::Path;
use std::process::Command;

use serde_json::json;

use fixture::{
    first_observation, other_platform, platform, report, stado, stderr, storage, DECLARATION,
    SERVICE, TARGET,
};

#[test]
fn the_report_reads_this_machine_and_says_it_changed_nothing() {
    let storage = storage(platform());
    let output = stado(
        storage.path(),
        &["repair", SERVICE, "--target", TARGET, "--json"],
    );
    assert!(output.status.success(), "{}", stderr(&output));

    let report = report(&output);
    assert_eq!(report["declaration"], DECLARATION);
    assert_eq!(report["service"], SERVICE);
    assert_eq!(report["target"], TARGET);
    assert_eq!(report["applied"], json!(false), "a report mutates nothing");

    let steps = report["steps"]
        .as_array()
        .expect("the report carries steps");
    assert!(!steps.is_empty(), "the service declares repair steps");
    for step in steps {
        assert_eq!(step["status"], "planned", "nothing ran: {step}");
    }

    let observation = first_observation(&report);
    // The platform is detected from the machine, not read back from the
    // declaration: this is the reading that makes the report a real one.
    assert_eq!(
        observation["release_platform"],
        platform(),
        "the observation reports the platform this test is running on"
    );
    assert_eq!(observation["release_platform_verdict"], "matched");
    assert_eq!(observation["target"], TARGET);
}

#[test]
fn the_observation_contradicts_a_declaration_that_is_wrong_about_this_machine() {
    let storage = storage(other_platform());
    let output = stado(
        storage.path(),
        &["repair", SERVICE, "--target", TARGET, "--json"],
    );
    assert!(output.status.success(), "{}", stderr(&output));

    let report = report(&output);
    let observation = first_observation(&report);
    assert_eq!(observation["declared_release_platform"], other_platform());
    assert_eq!(observation["release_platform"], platform());
    assert_eq!(
        observation["release_platform_verdict"], "mismatched",
        "a declaration the machine contradicts is reported as mismatched"
    );
}

#[test]
fn the_observation_agrees_with_the_filesystem_and_the_installed_binary() {
    let storage = storage(platform());
    let output = stado(
        storage.path(),
        &["repair", SERVICE, "--target", TARGET, "--json"],
    );
    assert!(output.status.success(), "{}", stderr(&output));

    let report = report(&output);
    let observation = first_observation(&report);

    // The Cargo home the observation describes is a directory on this machine,
    // so its owner, mode and size are checkable facts rather than reported ones.
    let home = std::env::var("HOME").expect("the test process has a home");
    let cargo_home = Path::new(&home).join(".cargo");
    let described = &observation["cargo"]["home"];
    assert_eq!(described["name"], ".cargo");
    match std::fs::metadata(&cargo_home) {
        Ok(metadata) => {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(described["kind"], "directory");
            assert_eq!(described["uid"], json!(metadata.uid()));
            assert_eq!(described["gid"], json!(metadata.gid()));
            assert_eq!(
                described["mode"],
                json!(format!("{:o}", metadata.mode() & 0o777)),
                "the reported mode is the one the filesystem carries"
            );
            assert_eq!(described["bytes"], json!(metadata.len()));
        }
        Err(_) => assert_ne!(
            described["kind"], "directory",
            "a directory reported on a machine that does not have it: {described}"
        ),
    }

    // The version the observation reports for the managed Stado binary is the
    // one that binary prints, or the observation says it is not installed.
    let binaries = observation["managed_binaries"]
        .as_array()
        .expect("the observation lists the managed binaries it looked for");
    let managed = binaries
        .iter()
        .find(|binary| binary["name"] == "stado")
        .expect("the managed inventory covers the Stado binary itself");
    let installed = Command::new("stado").arg("--version").output();
    match (managed["version"].as_str(), installed) {
        (Some(reported), Ok(output)) if output.status.success() => {
            assert_eq!(
                reported.trim(),
                String::from_utf8_lossy(&output.stdout).trim(),
                "the reported version is not what the installed binary prints"
            );
            assert_eq!(managed["state"], "present");
        }
        (Some(reported), _) => {
            panic!("a version was reported for a binary that cannot run: {reported}")
        }
        (None, Ok(output)) if output.status.success() => {
            panic!("no version was reported while the binary runs: {managed}")
        }
        (None, _) => assert_ne!(managed["state"], "present"),
    }
}

#[test]
fn a_target_outside_the_registry_is_refused_instead_of_reported() {
    let storage = storage(platform());
    let output = stado(
        storage.path(),
        &["repair", SERVICE, "--target", "nowhere-host"],
    );

    assert_eq!(
        output.status.code(),
        Some(1),
        "an unknown host is a wrong request: {}",
        stderr(&output)
    );
    assert!(
        stderr(&output).contains("target 'nowhere-host' is not in the canonical registry"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn a_service_or_step_nobody_declared_is_refused() {
    let storage = storage(platform());

    let service = stado(
        storage.path(),
        &["repair", "no-such-service", "--target", TARGET],
    );
    assert_eq!(service.status.code(), Some(1), "{}", stderr(&service));
    assert!(
        stderr(&service).contains(&format!(
            "no-such-service declares no repair; add it to {DECLARATION}."
        )),
        "{}",
        stderr(&service)
    );

    let step = stado(
        storage.path(),
        &["repair", SERVICE, "--step", "invented", "--target", TARGET],
    );
    assert_eq!(step.status.code(), Some(1), "{}", stderr(&step));
    assert!(
        stderr(&step).contains(&format!(
            "{SERVICE} declares no repair step invented; add it to {DECLARATION}."
        )),
        "{}",
        stderr(&step)
    );
}

#[test]
fn the_declaration_readers_refuse_the_flags_that_belong_to_a_run() {
    let storage = storage(platform());

    let listing = stado(storage.path(), &["repair", "list", "--apply"]);
    assert_eq!(listing.status.code(), Some(2), "{}", stderr(&listing));
    assert!(
        stderr(&listing)
            .contains("repair list accepts only its documented declaration filters and --json."),
        "{}",
        stderr(&listing)
    );

    let show = stado(storage.path(), &["repair", "show", SERVICE]);
    assert_eq!(show.status.code(), Some(2), "{}", stderr(&show));
    assert!(
        stderr(&show).contains("repair show requires SERVICE and STEP."),
        "{}",
        stderr(&show)
    );
}

#[test]
fn every_declared_step_states_its_mode_and_its_proof() {
    let storage = storage(platform());
    let output = stado(storage.path(), &["repair", "list", "--json"]);
    assert!(output.status.success(), "{}", stderr(&output));

    let listing = report(&output);
    assert_eq!(listing["declaration"], DECLARATION);
    let services = listing["services"].as_array().expect("declared services");
    let mut steps = 0;
    for service in services {
        for step in service["repair"].as_array().into_iter().flatten() {
            assert!(step["name"].is_string(), "a step without a name: {step}");
            assert!(
                step["mutating"].is_boolean(),
                "a step without a mode: {step}"
            );
            assert!(
                step["proof"]
                    .as_str()
                    .is_some_and(|proof| !proof.is_empty()),
                "a step that states no proof: {step}"
            );
            steps += 1;
        }
    }
    assert!(steps > 0, "the catalogue declares repair steps");
}
