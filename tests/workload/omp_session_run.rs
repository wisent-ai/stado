//! A started `omp-session` clones and builds below the host's declared work
//! root, never under the home, and the janitor judges what it left on this
//! platform's own code paths.
//!
//! One isolated deployment (`config init`: a local registry naming this
//! machine and a local store) and the real `stado serve --worker`. A bare
//! repository is the origin, and the caller's clean, fully pushed clone of
//! it sits where an operator keeps his checkouts, under
//! `~/Documents/CodingProjects/Wisent`. While the host declares no work
//! root, `stado workload start omp-session` is refused naming the host and
//! `stado space root <host> --path`, before anything is queued. With a work
//! root declared it queues the session; its declared job secrets name vault
//! roles an isolated home holds no grant for, so no worker here can claim
//! that job: the journey cancels it and submits the very command it
//! recorded, which the worker runs as it runs a session. What the command
//! left is read back: the clone in `<work root>/session-checkouts/
//! <repository>` holding the clone marker that names the job, the job's
//! retained log naming the Cargo target and temporary directory inside the
//! job's work tree below the work root, and no new entry at the top of the
//! home. With a file where the session checkouts directory belongs, the same
//! command fails saying so and naming `stado space root <host> --path`.
//! Last, `stado space reclaim --local` previews the janitor here: the
//! caller's checkout is kept as `not_session_checkout`; the session's
//! checkout holding a commit only a local tag reaches is kept as
//! `unpushed_work` naming that commit, and once the tag is gone it is
//! eligible. The report keeps the source revision and every command with
//! its exit status and streams.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::Receiver;

use serde_json::{json, Value};
use stado::providers::local::space_reclaim::queue_workdirs::WORKDIR_PREFIX;
use stado::providers::local::work_base::{
    JOBS_LEAF, SESSION_CHECKOUTS_LEAF, SESSION_CLONE_MARKER,
};

use crate::deployment::{line_with_any, Deployment};

/// The repository the session is asked to work in.
const WORKSPACE: &str = "sample-repository";
/// The run the session's command is submitted under, and the run it is
/// submitted under again with its checkouts directory unwritable.
const SESSION_RUN: &str = "omp-session-workspace-run";
const REFUSED_RUN: &str = "omp-session-workspace-refused";
/// The janitor's report section for session checkouts.
const CLEANER: &str = "session_checkouts";

/// Git with none of this machine account's configuration.
fn git(directory: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(["-c", "user.name=Stado session fixture"])
        .args(["-c", "user.email=stado-session@example.invalid"])
        .args(["-c", "init.defaultBranch=main"])
        .args(args)
        .current_dir(directory)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "true")
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} in {}: {}",
        directory.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// A bare repository holding one commit, and the caller's clone of it.
fn origin_and_caller(home: &Path) -> PathBuf {
    let seed = home.join("fixtures/seed");
    fs::create_dir_all(&seed).unwrap();
    git(&seed, &["init", "--quiet"]);
    fs::write(seed.join("README"), "omp-session workspace fixture\n").unwrap();
    git(&seed, &["add", "README"]);
    git(&seed, &["commit", "--quiet", "-m", "seed"]);
    let bare = home.join("fixtures/origin.git");
    let (seed, bare) = (seed.to_str().unwrap(), bare.to_str().unwrap());
    git(home, &["clone", "--quiet", "--bare", seed, bare]);
    let caller = home.join("Documents/CodingProjects/Wisent").join(WORKSPACE);
    fs::create_dir_all(caller.parent().unwrap()).unwrap();
    git(home, &["clone", "--quiet", bare, caller.to_str().unwrap()]);
    caller
}

/// The names at the top of the home.
fn top_of(home: &Path) -> BTreeSet<String> {
    fs::read_dir(home)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

/// The record of the job submitted as `run` once it ended, and whether it
/// ended failed.
fn ended(deployment: &mut Deployment, lines: &Receiver<String>, run: &str) -> (Value, bool) {
    for line in lines.iter() {
        for (prefix, failed) in [("completed", false), ("failed", true)] {
            let found = deployment
                .documents(prefix)
                .into_iter()
                .find(|job| job["run_id"] == json!(run));
            if let Some(job) = found {
                return (job, failed);
            }
        }
        deployment.report["last_service_line"] = json!(line);
    }
    panic!("the service ended before the job of {run} ended; inspect service.stderr");
}

/// What a job said: its recorded error and its retained log.
fn said(deployment: &Deployment, job: &Value) -> String {
    let id = job["job_id"].as_str().expect("a job record names its id");
    let log = deployment
        .store()
        .join("status")
        .join(id)
        .join("output/command_output.log");
    let logged = fs::read_to_string(&log).unwrap_or_default();
    format!("{}\n{logged}", job["error"].as_str().unwrap_or_default())
}

/// The janitor's preview on this machine, and its session checkouts section.
fn preview(deployment: &mut Deployment) -> (Value, Value) {
    let report: Value = serde_json::from_str(&deployment.cli(&["space", "reclaim", "--local"]))
        .expect("the preview prints its report as JSON");
    let cleaner = report["cleaners"][CLEANER].clone();
    let previews = deployment.report["previews"].as_array_mut();
    previews.expect("the journey keeps its previews").push(report.clone());
    deployment.save();
    (report, cleaner)
}

/// `stado workload start omp-session` on `target`: whether it queued, its
/// standard output and its standard error.
fn start(deployment: &mut Deployment, target: &str) -> (bool, String, String) {
    deployment.run(
        &[
            "workload", "start", "omp-session", "--workspace", WORKSPACE, "--target", target,
            "--task", "list the repository's files", "--max-time", "1m", "--json",
        ],
        "",
    )
}

#[test]
fn a_session_works_below_the_work_root_and_the_janitor_judges_what_it_left() {
    let mut deployment = Deployment::start("omp-session-workspace");
    deployment.report["previews"] = json!([]);
    let home = deployment.root.clone();
    let registry: Value = serde_json::from_str(&deployment.cli(&["registry", "pull"])).unwrap();
    let target = registry["targets"][0]["name"]
        .as_str()
        .expect("the first-run registry names this machine")
        .to_string();
    let caller = origin_and_caller(&home);

    // No work root declared: refused before anything is queued, naming the
    // host and the verb that declares one.
    let (queued, _, refusal) = start(&mut deployment, &target);
    deployment.report["undeclared_refusal"] = json!(refusal);
    assert!(!queued, "a host with no work root was given a session");
    assert!(
        refusal.contains(&format!("{target} declares no work root"))
            && refusal.contains(&format!("stado space root {target} --path")),
        "the refusal does not name the host and the verb that declares a work root:\n{refusal}"
    );
    assert!(deployment.documents("queue").is_empty(), "a refused start queued a job");

    let work_root = home.join("work-root");
    fs::create_dir_all(&work_root).unwrap();
    let root = work_root.to_str().unwrap();
    let at = "targets.0.work_root";
    deployment.cli(&["registry", "set", "--path", at, "--value", root, "--json"]);
    // On macOS the janitor opens ~/Documents only for a program the host's
    // entry grants it to.
    let grants = json!([{
        "program": env!("CARGO_BIN_EXE_stado"),
        "folder": "documents",
        "reason": "the janitor judges the checkouts under ~/Documents/CodingProjects",
    }]);
    let grants = grants.to_string();
    let path = "targets.0.privacy_grants";
    deployment.cli(&["registry", "set", "--path", path, "--value", &grants, "--json"]);

    let lines = deployment.serve(&["--worker", "--kind", "local", "--poll-seconds", "1"]);
    let published = line_with_any(&lines, &[": published accepting_jobs="]);
    deployment.report["published"] = json!(published);
    assert!(
        published.contains("accepting_jobs=true"),
        "the worker on this host admits no work, so the journey cannot run here: {published}"
    );

    let (queued, record, refusal) = start(&mut deployment, &target);
    assert!(queued, "the session was not queued on {target}: {refusal}");
    let record: Value = serde_json::from_str(&record).expect("the start prints its record");
    let checkout = work_root.join(SESSION_CHECKOUTS_LEAF).join(WORKSPACE);
    assert_eq!(record["cwd"], json!(checkout.to_str().unwrap()), "{record}");
    let command = record["command"].as_str().expect("the record names its command");
    let command = command.to_string();
    let queued_job = record["job_id"].as_str().expect("the record names its job");
    deployment.run(&["cancel", queued_job], "");

    // Read once the start and the cancel wrote what they write, so what is
    // new at the top of the home afterwards is what the session wrote.
    let before = top_of(&home);
    deployment.cli(&["submit", "--run-id", SESSION_RUN, &command]);
    let (job, _) = ended(&mut deployment, &lines, SESSION_RUN);
    let after = top_of(&home);
    let job_id = job["job_id"].as_str().expect("a job record names its id").to_string();
    let words = said(&deployment, &job);
    deployment.report["session_job"] = json!({"record": job, "said": words});
    deployment.save();
    let tree = work_root.join(JOBS_LEAF).join(format!("{WORKDIR_PREFIX}{job_id}"));
    let placed = format!(
        "omp-session works in {}; job tree {}, Cargo target {}, temporary directory {}",
        checkout.display(),
        tree.display(),
        tree.join("cargo-target").display(),
        tree.join("tmp").display()
    );
    assert!(words.contains(&placed), "the session did not work where it must:\n{words}");
    assert!(checkout.join("README").is_file(), "no clone at {}", checkout.display());
    let marker = fs::read_to_string(checkout.join(".git").join(SESSION_CLONE_MARKER))
        .expect("the session wrote its clone marker");
    assert_eq!(marker.trim(), job_id, "the clone marker names another job");
    let new: Vec<&String> = after.difference(&before).collect();
    assert!(new.is_empty(), "the session wrote at the top of the home: {new:?}");
    assert_eq!(git(&caller, &["status", "--porcelain"]), "", "the caller's checkout changed");

    // A file where the session checkouts directory belongs: nothing can be
    // cloned below it, and the job says so with the verb that declares
    // another work root.
    let checkouts = work_root.join(SESSION_CHECKOUTS_LEAF);
    let kept = work_root.join("session-checkouts-kept-aside");
    fs::rename(&checkouts, &kept).unwrap();
    fs::write(&checkouts, "a file where the session checkouts directory belongs\n").unwrap();
    deployment.cli(&["submit", "--run-id", REFUSED_RUN, &command]);
    let (refused, failed) = ended(&mut deployment, &lines, REFUSED_RUN);
    let refusal = said(&deployment, &refused);
    deployment.report["refused_job"] = json!({"record": refused, "said": refusal});
    assert!(failed, "a session whose checkouts directory is a file did not fail: {refusal}");
    assert!(
        refusal.contains(&format!("the session checkouts directory {}", checkouts.display()))
            && refusal.contains(&format!("stado space root {target} --path")),
        "the failure does not name the directory and the verb that declares a work root:\n{refusal}"
    );
    fs::remove_file(&checkouts).unwrap();
    fs::rename(&kept, &checkouts).unwrap();
    deployment.stop();

    // The janitor on this platform: a commit only a local tag reaches keeps
    // the session's checkout, and the caller's checkout no session cloned is
    // kept whatever git says of it.
    fs::write(checkout.join("HELD"), "work only a local tag reaches\n").unwrap();
    git(&checkout, &["add", "HELD"]);
    git(&checkout, &["commit", "--quiet", "-m", "held by a tag"]);
    let held = git(&checkout, &["rev-parse", "HEAD"]);
    git(&checkout, &["tag", "held-by-a-tag"]);
    git(&checkout, &["reset", "--quiet", "--hard", "@{upstream}"]);
    deployment.set("STADO_WORK_ROOT", root);
    let (report, cleaner) = preview(&mut deployment);
    assert_eq!(cleaner["skipped"]["not_session_checkout"], json!(1), "{report}");
    assert_eq!(cleaner["skipped"]["unpushed_work"], json!(1), "{report}");
    let errors = report["errors"].to_string();
    assert!(errors.contains(&held), "the tag-held commit is not named: {report}");

    git(&checkout, &["tag", "-d", "held-by-a-tag"]);
    let (report, cleaner) = preview(&mut deployment);
    assert_eq!(cleaner["skipped"]["not_session_checkout"], json!(1), "{report}");
    assert_eq!(cleaner["skipped"]["unpushed_work"], Value::Null, "{report}");
    assert_eq!(cleaner["eligible_items"], json!(1), "{report}");
    assert!(caller.join(".git").is_dir() && checkout.join(".git").is_dir());
    deployment.pass();
}
