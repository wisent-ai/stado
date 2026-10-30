//! A declared unit its init system keeps restarting is said to the operator
//! by the host's own health pass, through the real product.
//!
//! One isolated deployment runs the host process with the health role while
//! its registry declares one launchd job of this journey: a program that
//! writes one line to its stderr file and exits 3, under a label unique to
//! the run and a plist inside the deployment. The pass after the declaration
//! must read the job failing with its exit code, page the operator through
//! Stado's own alert channels with the unit and the line the job last wrote,
//! and say the page's answer in the unit's beacon `detail`. Oko is not
//! installed under the deployment's home, which the detail must say as
//! "Oko not installed", once. The loop register keeps both answers, so a
//! later pass neither pages nor asks Oko again. The job is booted out at the
//! end. macOS only: it drives launchd.
use crate::deployment::Deployment;
use serde_json::{json, Value};
use std::fs;
use std::process::Command;

/// The stored beacon of the deployment's one declared host, found through
/// `stado host beacon list` as the fleet finds it.
fn beacon(deployment: &mut Deployment) -> Value {
    let registry: Value =
        serde_json::from_slice(&fs::read(deployment.store().join("registry.json")).unwrap())
            .unwrap();
    let declared = registry["targets"][0]["name"].as_str().unwrap().to_string();
    let ages: Value =
        serde_json::from_str(&deployment.cli(&["host", "beacon", "list", "--json"])).unwrap();
    let row = ages["hosts"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|row| row["host"] == declared)
        .cloned()
        .unwrap_or_else(|| panic!("beacon list must list {declared}: {ages}"));
    let path = deployment.store().join(
        row["beacon"]
            .as_str()
            .unwrap_or_else(|| panic!("beacon list names no beacon object: {row}")),
    );
    serde_json::from_slice(&fs::read(&path).unwrap()).expect("the beacon is JSON")
}

#[test]
fn a_crash_looping_unit_pages_the_operator_once_even_without_oko() {
    let mut deployment = Deployment::start();
    deployment.serve();
    let run = deployment
        .root
        .file_name()
        .and_then(|name| name.to_str())
        .expect("the run directory has a name")
        .to_string();
    let label = format!("com.wisent.stado-test.crash-loop-{run}");
    let name = format!("stado-crash-loop-{run}");
    let agents = deployment.home.join("Library/LaunchAgents");
    fs::create_dir_all(&agents).unwrap();
    let plist = agents.join(format!("{label}.plist"));
    let stderr = deployment.root.join("crash-loop.err");
    let line = format!("crash-loop journey {run}: the declaration this release requires is absent");
    fs::write(
        &plist,
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>{label}</string>
<key>ProgramArguments</key><array><string>/bin/sh</string><string>-c</string><string>echo '{line}' 1&gt;&amp;2; exit 3</string></array>
<key>StandardErrorPath</key><string>{}</string>
<key>RunAtLoad</key><true/>
<key>KeepAlive</key><false/>
</dict></plist>
"#,
            stderr.display()
        ),
    )
    .unwrap();
    let domain = format!("gui/{}", nix::unistd::getuid());
    let booted = Command::new("/bin/launchctl")
        .args(["bootstrap", &domain, plist.to_str().unwrap()])
        .output()
        .unwrap();
    deployment.report["bootstrap"] = json!({
        "exit_status": booted.status.code(),
        "stderr": String::from_utf8_lossy(&booted.stderr),
    });
    assert!(
        booted.status.success(),
        "the journey's job could not be loaded"
    );
    // The job ran once and exited 3 before the next pass reads it.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !fs::read_to_string(&stderr).is_ok_and(|text| text.contains(&line)) {
        assert!(
            std::time::Instant::now() < deadline,
            "the journey's job wrote nothing to {}",
            stderr.display()
        );
        std::thread::sleep(std::time::Duration::from_millis(200));
    }

    let declared = json!([{
        "name": name,
        "unit": "",
        "label": label,
        "path": plist,
        "kind": "launchd",
        "program": "/bin/sh",
        "args": ["-c", "exit 3"],
    }]);
    deployment.cli(&[
        "registry",
        "set",
        "--path",
        "targets.0.services",
        "--value",
        &declared.to_string(),
        "--json",
    ]);
    deployment.report["declared"] = declared;
    // A pass already running may have read the registry before the
    // declaration; the one after it read it.
    deployment.next_publication();
    deployment.next_publication();
    let published = beacon(&mut deployment);
    let unit = published["units"][&label].clone();
    deployment.report["unit"] = unit.clone();

    let bootout = Command::new("/bin/launchctl")
        .args(["bootout", &format!("{domain}/{label}")])
        .output()
        .unwrap();
    deployment.report["bootout"] = json!({"exit_status": bootout.status.code()});

    assert_eq!(unit["state"], "failed", "{published}");
    let detail = unit["detail"].as_str().unwrap_or_else(|| panic!("{unit}"));
    assert!(
        detail.contains("its last run exited 3"),
        "the verdict names the exit: {detail}"
    );
    assert!(
        detail.contains("operator page: "),
        "the pass says what paging the operator answered: {detail}"
    );
    assert!(
        detail.contains("Oko defect: Oko not installed"),
        "a host without Oko says so: {detail}"
    );

    // The register keeps the loop with both answers, so no later pass asks
    // Oko again; the page is kept with what the channels answered.
    let register: Value = serde_json::from_slice(
        &fs::read(deployment.home.join(".stado/failing-units.json")).unwrap(),
    )
    .unwrap();
    deployment.report["register"] = register.clone();
    let kept = &register[&label];
    assert_eq!(kept["reported"], true, "{register}");
    assert!(
        kept["report"]
            .as_str()
            .is_some_and(|report| report.starts_with("Oko not installed")),
        "{register}"
    );
    assert!(
        kept["alert"]
            .as_str()
            .is_some_and(|alert| !alert.is_empty()),
        "the page's answer is kept: {register}"
    );
    // With no channel the deployment chose, the page is said as not taken,
    // naming where to look; a channel that took it would say `paged:`.
    assert!(
        kept["alerted"] == json!(true)
            || kept["alert"]
                .as_str()
                .is_some_and(|alert| alert.contains("stado alerts channels")
                    || alert.contains("no alert channel took")),
        "an unsent page names why: {register}"
    );
    deployment.pass();
}
