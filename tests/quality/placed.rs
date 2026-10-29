//! A placed quality check runs on the host the placement chose, and a newer
//! revision's check of the same product cancels the older one.
//!
//! One isolated deployment: `config init` seeds a local registry naming this
//! machine and a local store, and the real `stado serve --worker` claims the
//! placed jobs, each running the worker's own Stado (`$STADO_BIN`), which is
//! the binary under test. A fixture product commits two revisions: the first's
//! gate holds its check running, the second's passes at once. The first
//! check is placed and followed in the background; once its job runs, the
//! second check is placed. It must cancel the first job, run alone after it
//! and pass, and the first command must end non-zero saying its check ended
//! cancelled. Oko is left off `PATH`, so no ruling of the operator's governs
//! the fixture's gates.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde_json::{json, Value};

use crate::deployment::{line_with_any, Deployment};

const PRODUCT: &str = "example-placed-check";

/// `PATH` without every directory that holds an `oko`.
fn path_without_oko() -> String {
    let path = std::env::var_os("PATH").expect("PATH is set");
    let kept: Vec<PathBuf> = std::env::split_paths(&path)
        .filter(|directory| !directory.join("oko").exists())
        .collect();
    std::env::join_paths(kept)
        .expect("PATH joins")
        .into_string()
        .expect("PATH is UTF-8")
}

fn git(directory: &std::path::Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Stado placed check fixture")
        .env("GIT_AUTHOR_EMAIL", "")
        .env("GIT_COMMITTER_NAME", "Stado placed check fixture")
        .env("GIT_COMMITTER_EMAIL", "")
        .args(args)
        .current_dir(directory)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Commit the fixture's manifest with `gate` as its one quality gate.
fn commit(checkout: &std::path::Path, gate: Value, message: &str) -> String {
    let own: Value = serde_json::from_slice(
        &fs::read(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.wisent-release.json"))
            .unwrap(),
    )
    .unwrap();
    let manifest = json!({
        "schema_version": own["schema_version"],
        "product": PRODUCT,
        "releases": true,
        "version_source": {"kind": "text", "path": "VERSION"},
        "platforms": {
            "linux-amd64": {
                "runner_platform": "linux-amd64",
                "quality": [gate],
                "build": {"argv": ["cat", "VERSION"]},
                "stage": {"VERSION": "VERSION"}
            }
        },
        "promotion": {"channels": ["candidate"], "reconcile": false}
    });
    fs::write(
        checkout.join(".wisent-release.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    git(checkout, &["add", "VERSION", ".wisent-release.json"]);
    git(checkout, &["commit", "--quiet", "-m", message]);
    git(checkout, &["rev-parse", "HEAD"])
}

#[test]
fn a_newer_revisions_check_cancels_the_older_one_and_runs_alone() {
    let mut deployment = Deployment::start("quality-placed");
    deployment.set("PATH", &path_without_oko());
    let checkout = deployment.root.join(PRODUCT);
    fs::create_dir_all(&checkout).unwrap();
    git(&checkout, &["init", "--quiet", "--initial-branch=main"]);
    fs::write(checkout.join("VERSION"), "0.1.0\n").unwrap();
    // The first revision's gate holds its check running for many of the
    // worker's one-second polls; the second's passes at once.
    let held = commit(
        &checkout,
        json!({"name": "hold", "kind": "format", "argv": ["perl", "-e", "select(undef, undef, undef, 600)"]}),
        "a gate that holds the check",
    );
    let quick = commit(
        &checkout,
        json!({"name": "pass", "kind": "format", "argv": ["true"]}),
        "a gate that passes",
    );
    let root = checkout.to_string_lossy().into_owned();

    let lines = deployment.serve(&["--worker", "--kind", "local", "--poll-seconds", "1"]);
    // The agent's loop line once its capacity is in the store, which the
    // placement reads.
    let published = line_with_any(&lines, &[": published accepting_jobs="]);
    deployment.report["published"] = json!(published);

    let first = deployment
        .native(std::path::Path::new(env!("CARGO_BIN_EXE_stado")))
        .args([
            "quality",
            "check",
            "--root",
            root.as_str(),
            "--revision",
            held.as_str(),
            "--place",
            "--json",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let started = line_with_any(&lines, &["Started job", "ordinary work remains blocked"]);
    assert!(
        started.contains("Started job"),
        "the worker on this host refuses ordinary work, so the journey cannot run here: {started}"
    );
    deployment.report["first_started"] = json!(started);
    let held_job = deployment
        .documents("running")
        .into_iter()
        .find(|job| job["batch_id"] == "quality-check")
        .expect("the first check's job runs");
    let held_id = held_job["job_id"].as_str().unwrap().to_string();

    let (second_passed, second_out, second_err) = deployment.run(
        &[
            "quality",
            "check",
            "--root",
            &root,
            "--revision",
            &quick,
            "--place",
            "--json",
        ],
        "",
    );
    let first = first.wait_with_output().unwrap();
    let first_err = String::from_utf8_lossy(&first.stderr).into_owned();
    deployment.report["first"] = json!({
        "exit_status": first.status.code(),
        "stdout": String::from_utf8_lossy(&first.stdout),
        "stderr": first_err,
    });
    deployment.save();

    assert!(
        second_passed,
        "the newer revision's check did not pass: {second_err}"
    );
    let answer: Value = serde_json::from_str(&second_out).expect("the second check answers JSON");
    assert_eq!(answer["status"], "passed", "{answer}");
    assert_eq!(answer["superseded"], json!([held_id]), "{answer}");
    assert!(
        second_err.contains(&format!("cancelled job {held_id} (running on "))
            && second_err.contains(&format!(
                "the check of {PRODUCT} {held} this check of {quick} supersedes"
            )),
        "the second check does not say which check it superseded: {second_err}"
    );
    assert!(
        !first.status.success(),
        "the superseded check exited 0: {first_err}"
    );
    assert!(
        first_err.contains(&format!("the placed check of {PRODUCT} {held} on "))
            && first_err.contains("ended cancelled"),
        "the superseded check does not say it ended cancelled: {first_err}"
    );
    let cancelled = deployment.documents("cancelled");
    assert!(
        cancelled
            .iter()
            .any(|job| job["job_id"] == held_id.as_str()),
        "the older check's job is not in cancelled/: {cancelled:?}"
    );
    let completed = deployment.documents("completed");
    assert!(
        completed
            .iter()
            .any(|job| job["job_id"] == answer["job_id"]),
        "the newer check's job is not in completed/: {completed:?}"
    );
    deployment.pass();
}
