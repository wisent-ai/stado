//! A job whose lease lapsed while its process still ran is put back in the
//! queue; the worker that runs it takes it back and never starts it a second
//! time beside itself.
//!
//! One isolated deployment: `config init` seeds a local store, a CPU job that
//! runs for many of the worker's poll periods is submitted, and the real
//! `stado serve` runs the worker. Once the job runs, its running document is
//! put back in queue/ as a lease expiry leaves it — state queued, the reason
//! `worker lease expired`, no lease — while its process goes on. The worker
//! must log that it took the job back, the document must be running again
//! with the attempt's start time and a lease, the job's process must have been
//! started exactly once, and the job must complete.
use crate::deployment::{line_with_any, Deployment};
use serde_json::json;
use std::fs;

#[test]
fn a_requeued_job_its_worker_still_runs_is_taken_back_not_started_twice() {
    let mut deployment = Deployment::start("readopt");
    let command = "perl -e 'select(undef, undef, undef, 20)'";
    deployment.cli(&["submit", "--run-id", "readopt-journey", command]);
    let lines = deployment.serve(&["--worker", "--kind", "local", "--poll-seconds", "1"]);

    let first = line_with_any(&lines, &["Started job", "ordinary work remains blocked"]);
    assert!(
        first.contains("Started job"),
        "the worker on this host refuses ordinary work, so the journey cannot run here: {first}"
    );
    deployment.report["started"] = json!(first);
    let running = deployment
        .documents("running")
        .into_iter()
        .next()
        .expect("the claimed job is in running/");
    let job_id = running["job_id"]
        .as_str()
        .expect("the running job names its id")
        .to_string();
    let started_at = running["started_at"].clone();
    deployment.report["running_job"] = running.clone();

    // What a lease expiry leaves: the record back in queue/, its process
    // untouched.
    let mut requeued = running.clone();
    requeued["state"] = json!("queued");
    requeued["error"] = json!("worker lease expired");
    requeued
        .as_object_mut()
        .expect("a job is an object")
        .remove("lease_expires_at");
    let store = deployment.store();
    fs::create_dir_all(store.join("queue")).unwrap();
    fs::write(
        store.join("queue").join(format!("{job_id}.json")),
        serde_json::to_vec_pretty(&requeued).unwrap(),
    )
    .unwrap();
    fs::remove_file(store.join("running").join(format!("{job_id}.json"))).unwrap();
    deployment.report["requeued_job"] = requeued;

    let started_line = format!("Started job {job_id}");
    let taken = line_with_any(
        &lines,
        &[&format!("{job_id}: already running here"), &started_line],
    );
    deployment.report["taken_back"] = json!(taken);
    assert!(
        taken.contains("took the job back") && taken.contains("no second process was started"),
        "the worker takes the job back instead of starting it again: {taken}"
    );
    let again = deployment
        .documents("running")
        .into_iter()
        .find(|job| job["job_id"] == json!(job_id))
        .expect("the job is running again");
    assert_eq!(
        again["started_at"], started_at,
        "the attempt keeps its start: {again}"
    );
    assert!(
        again["lease_expires_at"].is_string(),
        "the job carries a lease again: {again}"
    );
    deployment.report["running_again"] = again;

    for line in lines.iter() {
        assert!(
            !line.contains(&started_line),
            "the job's process was started a second time: {line}"
        );
        if deployment
            .documents("completed")
            .iter()
            .any(|job| job["job_id"] == json!(job_id))
        {
            break;
        }
    }
    let service = fs::read_to_string(deployment.root.join("service.stdout")).unwrap()
        + &fs::read_to_string(deployment.root.join("service.stderr")).unwrap();
    let first_start = service.find(&started_line);
    assert!(
        first_start.is_some() && first_start == service.rfind(&started_line),
        "the job's process was started exactly once"
    );
    deployment.report["completed_job"] = deployment
        .documents("completed")
        .into_iter()
        .find(|job| job["job_id"] == json!(job_id))
        .expect("the job completes");
    deployment.pass();
}
