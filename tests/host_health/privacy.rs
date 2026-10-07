//! The host process measures which protected home folders it may read, every
//! beacon carries that measurement, and `stado host privacy` reads it back
//! and judges it against the grants the registry declares for the host.
//! One isolated deployment whose HOME holds a readable Documents, no Desktop
//! and a Downloads this user may not list; `stado serve` runs as the real
//! product and publishes the beacon, and the CLI answers from it. A denial
//! macOS itself records (EPERM from its privacy controls) cannot be clicked
//! from a test, so the refusal is exercised through a beacon carrying one,
//! written into the deployment's own store.
mod deployment;

use deployment::Deployment;
use serde_json::{json, Value};
use std::fs;
use std::os::unix::fs::PermissionsExt;

#[test]
fn the_host_process_reports_what_it_may_read_and_the_cli_reads_it_back() {
    let mut deployment = Deployment::start();
    fs::create_dir_all(deployment.home.join("Documents")).unwrap();
    let downloads = deployment.home.join("Downloads");
    fs::create_dir_all(&downloads).unwrap();
    fs::set_permissions(&downloads, fs::Permissions::from_mode(0o000)).unwrap();
    let registry: Value =
        serde_json::from_slice(&fs::read(deployment.store().join("registry.json")).unwrap())
            .unwrap();
    let declared = registry["targets"][0]["name"].as_str().unwrap().to_string();

    let slug = deployment.serve();
    let answer: Value =
        serde_json::from_str(&deployment.cli(&["host", "privacy", &declared, "--json"])).unwrap();
    fs::set_permissions(&downloads, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(answer["host"], slug, "{answer}");
    let folders = &answer["privacy"]["folders"];
    assert_eq!(folders["documents"]["state"], "granted", "{answer}");
    assert_eq!(folders["desktop"]["state"], "absent", "{answer}");
    assert_eq!(folders["downloads"]["state"], "unreadable", "{answer}");
    assert!(
        folders["downloads"]["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("ermission denied")),
        "an unreadable folder carries the operating system's own error: {answer}"
    );
    assert!(
        answer["privacy"]["program"]
            .as_str()
            .is_some_and(|program| program.ends_with("stado")),
        "the measurement names the program macOS judges: {answer}"
    );
    let text = deployment.cli(&["host", "privacy", &declared]);
    assert!(text.contains("Documents    granted"), "{text}");
    assert!(text.contains("Desktop      absent"), "{text}");
    assert_eq!(
        answer["grants"],
        json!([]),
        "nothing is declared yet: {answer}"
    );

    // A beacon carrying a macOS denial.
    let ages: Value =
        serde_json::from_str(&deployment.cli(&["registry", "beacon-age", "--json"])).unwrap();
    let beacon_path = deployment.store().join(
        ages["hosts"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|row| row["host"] == declared)
            .and_then(|row| row["beacon"].as_str())
            .unwrap_or_else(|| panic!("beacon-age must name {declared}'s beacon: {ages}")),
    );
    // Stop the publisher first so it does not overwrite the written denial.
    if let Some(child) = &mut deployment.child {
        child.kill().unwrap();
        child.wait().unwrap();
    }
    deployment.child = None;
    let mut beacon: Value = serde_json::from_slice(&fs::read(&beacon_path).unwrap()).unwrap();
    beacon["privacy"]["folders"]["documents"] = json!({
        "state": "denied",
        "path": deployment.home.join("Documents"),
        "detail": "Operation not permitted (os error 1)",
    });
    fs::write(&beacon_path, serde_json::to_vec(&beacon).unwrap()).unwrap();

    // Undeclared, the denial is reported and fails nothing.
    let undeclared = deployment.cli(&["host", "privacy", &declared]);
    assert!(undeclared.contains("Documents    denied"), "{undeclared}");

    // Declared, the same denial fails the command, naming the folder, the
    // program, the declared reason and where the switch is. Only macOS
    // decides folder access per program, so a Linux host's registry refuses
    // the declaration itself.
    let program = answer["privacy"]["program"].as_str().unwrap().to_string();
    let registry_path = deployment.store().join("registry.json");
    let original = fs::read(&registry_path).unwrap();
    let mut registry: Value = serde_json::from_slice(&original).unwrap();
    let darwin = registry["targets"][0]["release_platform"]
        .as_str()
        .is_some_and(|platform| platform.starts_with("darwin"));
    registry["targets"][0]["privacy_grants"] = json!([{
        "program": program,
        "folder": "documents",
        "reason": "product sync builds from the canonical checkouts",
    }]);
    fs::write(
        &registry_path,
        serde_json::to_vec_pretty(&registry).unwrap(),
    )
    .unwrap();
    let refused = deployment.run(&["host", "privacy", &declared]);
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        !refused.status.success(),
        "a denied declared grant must fail the command"
    );
    if darwin {
        assert!(stderr.contains("Documents"), "{stderr}");
        assert!(stderr.contains(&program), "{stderr}");
        assert!(stderr.contains("canonical checkouts"), "{stderr}");
        assert!(stderr.contains("Files and Folders"), "{stderr}");
    } else {
        assert!(stderr.contains("privacy_grants"), "{stderr}");
        assert!(stderr.contains("darwin"), "{stderr}");
    }

    // A grant on a folder that is not one macOS gates is refused by the
    // registry before anything reads it.
    if darwin {
        registry["targets"][0]["privacy_grants"][0]["folder"] = json!("pictures");
        fs::write(
            &registry_path,
            serde_json::to_vec_pretty(&registry).unwrap(),
        )
        .unwrap();
        let malformed = deployment.run(&["host", "privacy", &declared]);
        let stderr = String::from_utf8_lossy(&malformed.stderr);
        assert!(
            !malformed.status.success(),
            "a malformed grant must be refused"
        );
        assert!(stderr.contains("privacy_grants[0].folder"), "{stderr}");
    }
    fs::write(&registry_path, &original).unwrap();

    // A host with no beacon at all is refused by name, not answered empty.
    let unknown = deployment.run(&["host", "privacy", "no-such-host"]);
    assert!(!unknown.status.success(), "an unknown host must be refused");

    deployment.report["privacy"] = answer;
    deployment.pass();
}
