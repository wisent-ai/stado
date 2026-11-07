//! `stado space reclaim TARGET` reaches the second look on this platform:
//! the first apply keeps an unheld tree and records the look, the next takes
//! it. A record that cannot be written, or that reads back other than
//! written, keeps the tree and says why and how to repair it, instead of
//! keeping it in silence on every apply. With the queue store silent,
//! `queue_workdirs` judges a job's work tree by the same two looks.
//!
//! One isolated deployment per case (`config init` seeds a registry naming
//! this machine, with HOME the run directory), compiled into the scheduler
//! test binary so it runs on every platform the release gates run; the
//! reclaim program reaches this machine by the host channel's local route.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use serde_json::{json, Value};

use crate::deployment::Deployment;

/// An isolated deployment holding one abandoned build tree, and the host
/// name its registry gives this machine.
fn abandoned(name: &'static str) -> (Deployment, String, PathBuf) {
    let mut deployment = Deployment::start(name);
    let host = deployment
        .cli(&["registry", "self", "--name-only"])
        .trim()
        .to_string();
    // The isolated scan ceiling keeps the gate host's own load from
    // skipping the pass.
    let ceiling = f64::MAX.to_string();
    deployment.cli(&["config", "set", "host.scan_load_ceiling", &ceiling]);
    let tree = deployment.root.join(".stado/build-work/abandoned");
    fs::create_dir_all(&tree).unwrap();
    fs::write(tree.join("output"), b"build output\n").unwrap();
    (deployment, host, tree)
}

/// One pass of `stage`, applied or previewed; the whole receipt, recorded
/// in the case's report under `step`.
fn reclaim(deployment: &mut Deployment, host: &str, stage: &str, apply: bool, step: &str) -> Value {
    let mode = if apply { "--apply" } else { "--dry-run" };
    let mut args = vec!["space", "reclaim", host, "--stage", stage, mode, "--json"];
    if apply {
        args.extend(["--reason", "second look journey"]);
    }
    let report: Value = serde_json::from_str(&deployment.cli(&args))
        .expect("space reclaim --json answers one JSON document");
    deployment.report[step] = report.clone();
    deployment.save();
    report
}

/// The row of `stage` in a receipt.
fn row(report: &Value, stage: &str) -> Value {
    report["stages"]
        .as_array()
        .expect("the receipt lists its stages")
        .iter()
        .find(|row| row["stage"] == stage)
        .unwrap_or_else(|| panic!("the receipt has no {stage} row: {report}"))
        .clone()
}

/// One applied `build_scratch` pass; its row.
fn apply(deployment: &mut Deployment, host: &str, step: &str) -> Value {
    row(
        &reclaim(deployment, host, "build_scratch", true, step),
        "build_scratch",
    )
}

fn lines(stage: &Value, field: &str) -> Vec<String> {
    let list = stage[field].as_array().cloned().unwrap_or_default();
    list.iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

fn refused_with(stage: &Value, words: &[&str]) -> bool {
    lines(stage, "refused")
        .iter()
        .any(|line| line.contains("abandoned") && words.iter().all(|word| line.contains(word)))
}

/// The record directory of the second look under the isolated home.
fn settled_dir(deployment: &Deployment) -> PathBuf {
    deployment
        .root
        .join(".stado/work/host-reclaim-local-evidence/settled")
}

/// The one second-look record the first apply wrote.
fn only_record(deployment: &Deployment) -> PathBuf {
    let records: Vec<PathBuf> = fs::read_dir(settled_dir(deployment))
        .expect("the first apply wrote the second-look record")
        .map(|entry| entry.unwrap().path())
        .collect();
    let [record] = records.as_slice() else {
        panic!("one record for one tree: {records:?}");
    };
    record.clone()
}

/// A directory closed to every reader for as long as this lives, opened
/// again however the case ends, so the retained evidence stays readable.
struct Closed(PathBuf);

impl Closed {
    fn new(path: &std::path::Path) -> Self {
        fs::set_permissions(path, fs::Permissions::from_mode(0)).unwrap();
        Self(path.to_path_buf())
    }
}

impl Drop for Closed {
    fn drop(&mut self) {
        if let Err(error) = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o700)) {
            eprintln!("{} stays closed to every reader: {error}", self.0.display());
        }
    }
}

#[test]
fn an_unheld_tree_is_kept_on_the_first_apply_and_taken_on_the_next() {
    let (mut deployment, host, tree) = abandoned("second-look-settles");
    let first = apply(&mut deployment, &host, "first_apply");
    assert!(refused_with(&first, &["unheld on this look"]), "{first}");
    assert!(tree.is_dir(), "a tree was taken on its first look");
    let second = apply(&mut deployment, &host, "second_apply");
    assert!(
        lines(&second, "paths")
            .iter()
            .any(|path| path.contains("abandoned")),
        "the settled tree was not taken on the next apply: {second}"
    );
    assert!(!tree.exists(), "the settled unheld tree was kept");
    deployment.pass();
}

/// APFS keeps no time before 1970, so the case runs where the file system
/// does.
#[cfg(target_os = "linux")]
#[test]
fn a_tree_dated_before_1970_settles_like_any_other() {
    let (mut deployment, host, tree) = abandoned("second-look-before-epoch");
    let before = std::time::UNIX_EPOCH - std::time::Duration::from_secs(1);
    fs::File::open(&tree)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(before))
        .unwrap();
    let first = apply(&mut deployment, &host, "first_apply");
    assert!(refused_with(&first, &["unheld on this look"]), "{first}");
    let look = fs::read_to_string(only_record(&deployment)).unwrap();
    assert_eq!(
        look.trim(),
        "-1",
        "stat's answer for the tree was not the time before 1970"
    );
    let second = apply(&mut deployment, &host, "second_apply");
    assert!(
        !tree.exists(),
        "a tree dated before 1970 never settled: {second}"
    );
    deployment.pass();
}

#[test]
fn a_second_look_record_that_cannot_be_written_keeps_the_tree_and_says_why() {
    let (mut deployment, host, tree) = abandoned("second-look-unwritable");
    // The record's directory is a file, so no record can be written there.
    let blocker = settled_dir(&deployment);
    fs::create_dir_all(blocker.parent().unwrap()).unwrap();
    fs::write(&blocker, b"not a directory\n").unwrap();
    let refused = apply(&mut deployment, &host, "unwritable_apply");
    let record = blocker.display().to_string();
    let repair = format!("or {record} not a directory this account can write), then apply again");
    assert!(
        refused_with(
            &refused,
            &[
                "the second-look record",
                &record,
                "cannot be written (",
                &repair
            ]
        ),
        "an unwritable record did not keep the tree naming the record and the repair: {refused}"
    );
    assert!(tree.is_dir(), "a tree was taken with no look recorded");
    deployment.report["record_path"] = json!(record);
    deployment.pass();
}

#[test]
fn a_second_look_record_that_reads_back_otherwise_keeps_the_tree_and_says_why() {
    let (mut deployment, host, tree) = abandoned("second-look-read-back");
    let first = apply(&mut deployment, &host, "first_apply");
    assert!(refused_with(&first, &["unheld on this look"]), "{first}");
    // The record now swallows what is written to it, so the next apply finds
    // no earlier look, writes one and reads nothing back.
    let record = only_record(&deployment);
    fs::remove_file(&record).unwrap();
    std::os::unix::fs::symlink("/dev/null", &record).unwrap();
    let refused = apply(&mut deployment, &host, "read_back_apply");
    let path = record.display().to_string();
    let repair = format!("retained: remove {path}, then apply again");
    assert!(
        refused_with(&refused, &["the second-look record", &path, "reads back ''", "not '", &repair]),
        "a record that reads back nothing did not keep the tree naming it and the repair: {refused}"
    );
    assert!(
        tree.is_dir(),
        "a tree was taken on a look that was not recorded"
    );
    deployment.pass();
}

/// With the queue store silent, a job's work tree is observed on the first
/// apply and found eligible on the next look. Every pass after the first
/// preview is checked to be in local-evidence mode before it runs, and the
/// last one is a preview: the stage's work roots include the host's `/tmp`,
/// whose job trees an isolated deployment must not take.
#[test]
fn a_job_work_tree_is_observed_then_eligible_while_the_queue_store_is_silent() {
    let (mut deployment, host, _) = abandoned("second-look-queue-workdirs");
    // A prefix the stage asks the store for cannot be listed, so the store
    // cannot say which jobs are live. (A file there would list as empty, and
    // an empty keep-list is store mode; the preview below refuses to go on
    // unless the stage says the store did not answer.)
    let running = deployment.store().join("running");
    fs::create_dir_all(&running).unwrap();
    let _closed = Closed::new(&running);
    let tree = deployment.root.join(".stado/work/jobs/wc-second-look-job");
    fs::create_dir_all(&tree).unwrap();
    fs::write(tree.join("output"), b"job output\n").unwrap();
    let evidence = |report: &Value, decision: &str| {
        let row = row(report, "queue_workdirs");
        let rows = row["local_terminality_evidence"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        rows.iter().any(|seen| {
            seen["decision"] == decision
                && seen["path"]
                    .as_str()
                    .is_some_and(|path| path.contains("wc-second-look-job"))
        })
    };
    let preview = reclaim(
        &mut deployment,
        &host,
        "queue_workdirs",
        false,
        "first_preview",
    );
    assert!(
        preview.to_string().contains("queue_authority"),
        "the queue store answered, so the stage is not in local-evidence mode: {preview}"
    );
    let first = reclaim(
        &mut deployment,
        &host,
        "queue_workdirs",
        true,
        "first_apply",
    );
    assert!(evidence(&first, "observation_started"), "{first}");
    assert!(
        tree.is_dir(),
        "a work tree was taken on its first observation"
    );
    let next = reclaim(
        &mut deployment,
        &host,
        "queue_workdirs",
        false,
        "second_preview",
    );
    assert!(
        evidence(&next, "eligible"),
        "the second observation did not find it eligible: {next}"
    );
    assert!(tree.is_dir(), "a preview took the work tree");
    deployment.pass();
}
