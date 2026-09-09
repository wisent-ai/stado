//! Real `stado builds` recipe → poller → worker → artifact journey.
//!
//! The built Stado binary writes the recipe to an isolated canonical registry,
//! a real coordinator observes the public repository branch, and a real Stado
//! worker claims the platform-constrained job. The assertion reads the uploaded
//! artifact and the reconciled recipe state; no scheduler, Git, worker, or
//! storage stand-in is used.
//!
//! It runs by default. Both dependencies are reachable from anywhere the suite
//! runs: the repository it polls is `https://github.com/wisent-ai/stado.git`,
//! which is public, and the worker is a `stado agent` this case starts itself
//! against its own store, on the machine running the case.
//!
//! Running it is what exposed the defect it now also defends against. The
//! coordinator's by-run reaper retires a finished build's run inside the same
//! tick that reconciles the recipe, deleting the `completed/` job blob; a
//! reconciliation that read only the live job prefixes therefore recorded a
//! build that had really succeeded as `failed`, with the reason "job record
//! disappeared; the worker never reported" and no artifacts. The run this case
//! asserts on is the durable one, so the retained outcome is what decides.

mod journey;

use std::fs;

use serde_json::Value;

use crate::journey::Journey;

const RECIPE: &str = "probierz-native-build";
fn build_platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("linux", "x86_64") => "linux-amd64",
        (os, arch) => panic!("build journey has no platform mapping for {os}-{arch}"),
    }
}
const SOURCE: &str = "https://github.com/wisent-ai/stado.git";

#[test]
fn build_recipe_polls_public_git_runs_on_matching_worker_and_publishes_artifact() {
    let platform = build_platform();
    let mut journey = Journey::new();

    let malformed = journey.invoke(&[
        "builds",
        "add",
        "--name",
        "bad-build",
        "--repo",
        "file:///not-public",
        "--branch",
        "main",
        "--command",
        "true",
        "--artifact",
        "out",
        "--platform",
        platform,
    ]);
    assert_eq!(malformed.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&malformed.stderr).contains("--repo must be an https:// clone URL")
    );

    let added = journey.invoke_ok(&[
        "builds",
        "add",
        "--name",
        RECIPE,
        "--repo",
        SOURCE,
        "--branch",
        "main",
        "--command",
        "printf 'built by stado\\n' > build-output.txt",
        "--artifact",
        "build-output.txt",
        "--platform",
        platform,
        "--interval-seconds",
        "1",
        "--json",
    ]);
    let added: Value = serde_json::from_slice(&added.stdout).unwrap();
    assert_eq!(added["enabled"], false);

    let duplicate = journey.invoke(&[
        "builds",
        "add",
        "--name",
        RECIPE,
        "--repo",
        SOURCE,
        "--branch",
        "main",
        "--command",
        "true",
        "--artifact",
        "out",
        "--platform",
        platform,
    ]);
    assert_eq!(duplicate.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&duplicate.stderr)
        .contains("build recipe \"probierz-native-build\" already exists"));

    journey.invoke_ok(&["builds", "enable", RECIPE]);
    journey.start_agent();
    journey.invoke_ok(&["coordinator", "--once"]);
    let submitted = journey.status();
    let run = &submitted["recipe"]["runs"][platform];
    assert_eq!(run["status"], "running", "{submitted}");
    let job_id = run["job_id"].as_str().unwrap();

    journey.wait_for_terminal_job(job_id);

    // The finished job's own uploaded output, read before the next tick: the
    // by-run reaper retires the run at the end of that tick and takes
    // `status/<job>/output/` with it, which is exactly why the recipe has to
    // record what a build produced during the same tick rather than after it.
    let destination = journey.home.path().join("results");
    journey.invoke_ok(&["results", job_id, destination.to_str().unwrap()]);
    assert_eq!(
        fs::read_to_string(destination.join("build-output.txt")).unwrap(),
        "built by stado\n"
    );

    journey.invoke_ok(&["coordinator", "--once"]);
    let completed = journey.status();
    let run = &completed["recipe"]["runs"][platform];
    assert_eq!(run["status"], "succeeded", "{completed}");
    assert_eq!(run["declared"], false);
    assert!(
        run["artifact_uris"]
            .as_array()
            .is_some_and(|items| !items.is_empty()),
        "a succeeded build recorded no artifacts: {completed}"
    );

    // Where the job landed, in the durable record that survives the reaper
    // which ran at the end of that same tick.
    let run_id = run["run_id"]
        .as_str()
        .expect("a submitted build records the durable run it belongs to");
    let manifest: Value = serde_json::from_str(
        &fs::read_to_string(journey.storage.join(format!("runs/{run_id}.json")))
            .expect("the durable run manifest is readable"),
    )
    .expect("the durable run manifest is JSON");
    let entry = manifest["entries"]
        .as_array()
        .expect("the manifest carries its entries")
        .iter()
        .find(|entry| entry["job_id"].as_str() == Some(job_id))
        .unwrap_or_else(|| panic!("run {run_id} has no entry for {job_id}: {manifest}"));
    assert_eq!(
        entry["outcome"]["prefix"], "completed",
        "the retained outcome must say where the job landed: {manifest}"
    );
    println!(
        "verified recipe={RECIPE}; job={job_id}; platform={platform}; artifact=build-output.txt"
    );
}
