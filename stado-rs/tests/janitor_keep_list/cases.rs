//! Which job trees a real `stado disk-cleanup` pass leaves on this machine.

use serde_json::json;

use crate::fixture::Journey;

/// The gate itself, on a population the product created.
///
/// One job runs to completion through a real local agent and one is still
/// running when the agent stops, so both trees under `~/.stado/work/jobs` and
/// both records in the store were written by the product. The pass then
/// reclaims the terminal job's tree and keeps the running job's, and the count
/// it reports matches what is left on disk.
///
/// The agent is stopped before the pass because the agent holds a shared
/// workload lock while a job runs, and a pass that answers `lock_busy` would
/// prove nothing about keep-lists. What the store says about each job is what
/// the janitor reads, and stopping the agent does not change that.
#[test]
fn a_terminal_jobs_tree_is_reclaimed_and_a_running_jobs_tree_is_kept() {
    let mut journey = Journey::new();
    let finished = journey.home().join("first.done");
    let started = journey.home().join("second.started");
    let release = journey.home().join("second.release");
    let quick = journey.submit("keep-list-quick", &format!(": > '{}'", finished.display()));
    let blocked = journey.submit(
        "keep-list-blocked",
        &format!(
            ": > '{}'; while [ ! -f '{}' ]; do /bin/sleep 0.1; done",
            started.display(),
            release.display()
        ),
    );
    journey.start_agent();
    journey.wait_for("the first job to reach a terminal record", |state| {
        state.recorded("completed", &quick)
    });
    journey.wait_for("the second job to start running", |state| {
        started.is_file() && state.recorded("running", &blocked)
    });
    journey.stop_agent();
    assert!(
        journey.workdir(&quick).is_dir() && journey.workdir(&blocked).is_dir(),
        "the agent must have left both job trees behind"
    );

    let report = journey.reclaim();

    let workdirs = &report["cleaners"]["queue_workdirs"];
    assert_eq!(workdirs["deleted_items"], 1, "reclaim report: {report:#}");
    assert_eq!(workdirs["skipped"]["job_queued_or_running"], 1);
    assert!(
        !journey.workdir(&quick).exists(),
        "the terminal job's tree must be reclaimed"
    );
    assert!(
        journey.workdir(&blocked).join("output").is_dir(),
        "a job the store still calls running must keep its tree"
    );
    std::fs::write(&release, b"go\n").expect("release the second workload");
}

/// The cost model, as the only thing about it an operator can see.
///
/// Job ids come from object NAMES. A keep-list that downloaded and parsed job
/// documents would drop a job whose body is unreadable — a blob mid-transition,
/// a truncated write, a document from a newer schema — and then delete the tree
/// a live job is writing into. The unreadable body is planted here rather than
/// submitted, because `stado submit` cannot write one.
#[test]
fn a_job_whose_document_cannot_be_read_keeps_its_workdir() {
    let journey = Journey::new();
    let fenced = "job-bbbbbbbbbbbbbbbbbbbbbbbb";
    let terminal = "job-cccccccccccccccccccccccc";
    journey.plant_record("running", fenced, "not a job document at all");
    journey.plant_record(
        "completed",
        terminal,
        &json!({"job_id": terminal, "state": "completed", "command": "true"}).to_string(),
    );
    let kept = journey.plant_workdir(fenced);
    let reclaimed = journey.plant_workdir(terminal);

    let report = journey.reclaim();

    assert_eq!(
        report["cleaners"]["queue_workdirs"]["skipped"]["job_queued_or_running"], 1,
        "reclaim report: {report:#}"
    );
    assert!(
        kept.join("output/payload.bin").is_file(),
        "a job named in running/ keeps its tree however unreadable its document is"
    );
    assert!(
        !reclaimed.exists(),
        "the terminal job's tree must still be reclaimed"
    );
}

/// An authority that cannot be read is not permission to delete.
///
/// With the `queue/` prefix unreadable, the pass cannot establish that any job
/// is terminal, so it removes nothing at all — including the tree of a job it
/// has a terminal record for — and says so in the cleaner's own counts.
#[test]
fn an_unreadable_queue_prefix_reclaims_nothing() {
    let journey = Journey::new();
    let terminal = "job-dddddddddddddddddddddddd";
    journey.plant_record(
        "completed",
        terminal,
        &json!({"job_id": terminal, "state": "completed", "command": "true"}).to_string(),
    );
    let tree = journey.plant_workdir(terminal);
    let queue = journey.store().join("queue");
    std::fs::create_dir_all(&queue).expect("the queue prefix");
    unreadable(&queue, true);

    let report = journey.reclaim();

    unreadable(&queue, false);
    let workdirs = &report["cleaners"]["queue_workdirs"];
    assert_eq!(workdirs["deleted_items"], 0, "reclaim report: {report:#}");
    assert_eq!(workdirs["skipped"]["queue_store_unreadable"], 1);
    assert!(
        tree.join("output/payload.bin").is_file(),
        "a pass that could not read the queue must delete no job tree"
    );
}

/// Take the read bit off a store prefix, and put it back.
fn unreadable(path: &std::path::Path, closed: bool) {
    use std::os::unix::fs::PermissionsExt;
    let mode = if closed { 0o000 } else { 0o700 };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .expect("the store prefix's mode is ours to set");
}
