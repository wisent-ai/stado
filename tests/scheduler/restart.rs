//! A worker that starts on a host settles the running jobs the worker it
//! replaced left behind: one whose process is gone is requeued at once and
//! runs again, instead of holding its record — and every schedule waiting on
//! it — until the dead worker's lease promise passes; one whose process still
//! runs in its work directory keeps its lease.
//!
//! One isolated deployment: `config init` seeds a local store and a CPU job
//! that writes its pid and then runs for many poll periods is submitted. The
//! real `stado serve` worker starts it and is then killed as a crash kills it,
//! leaving the workload in its own process group. A second worker must log
//! that the job is still run by that pid and leave it running. The workload is
//! then killed too, and a third worker must requeue the job with the reason
//! `its worker was replaced while it ran`, start it again and see it complete.
use crate::deployment::{line_with, line_with_any, Deployment};
use serde_json::json;
use std::fs;
use std::process::Command;

#[test]
fn a_new_worker_settles_the_jobs_its_predecessor_left_behind() {
    let mut deployment = Deployment::start("restart");
    let command = "echo $$ > workload.pid && exec perl -e 'select(undef, undef, undef, 30)'";
    deployment.cli(&["submit", "--run-id", "restart-journey", command]);
    let serve = ["--worker", "--kind", "local", "--poll-seconds", "1"];
    let lines = deployment.serve(&serve);
    let first = line_with_any(&lines, &["Started job", "ordinary work remains blocked"]);
    assert!(
        first.contains("Started job"),
        "the worker on this host refuses ordinary work, so the journey cannot run here: {first}"
    );
    let running = deployment
        .documents("running")
        .into_iter()
        .next()
        .expect("the claimed job is in running/");
    let job_id = running["job_id"]
        .as_str()
        .expect("the running job names its id")
        .to_string();
    deployment.report["running_job"] = running;
    let pid_file = deployment
        .root
        .join(".stado/work/jobs")
        .join(format!("wc-{job_id}"))
        .join("workload.pid");
    let pid = loop {
        if let Ok(text) = fs::read_to_string(&pid_file) {
            if !text.trim().is_empty() {
                break text.trim().to_string();
            }
        }
        line_with(&lines, "");
    };
    deployment.report["workload_pid"] = json!(pid);

    // The worker dies; its workload, in its own process group, runs on.
    deployment.stop();
    let lines = deployment.serve(&serve);
    let kept = line_with(&lines, &format!("init: {job_id}: left by the worker"));
    deployment.report["kept"] = json!(kept);
    assert!(
        kept.contains(&format!("still run here by pid {pid}")) && kept.contains("keeps its lease"),
        "a job whose process still runs keeps its lease: {kept}"
    );
    assert!(
        deployment
            .documents("running")
            .iter()
            .any(|job| job["job_id"] == json!(job_id)),
        "the job still runs"
    );

    // Now the workload is gone too.
    let killed = Command::new("kill").args(["-9", &pid]).status().unwrap();
    assert!(killed.success(), "the workload {pid} could not be ended");
    deployment.stop();
    let lines = deployment.serve(&serve);
    let requeued = line_with(&lines, &format!("init: {job_id}: requeued"));
    deployment.report["requeued"] = json!(requeued);
    assert!(
        requeued.contains("its worker was replaced while it ran"),
        "the job is requeued with its reason: {requeued}"
    );
    let again = line_with(&lines, &format!("Started job {job_id}"));
    deployment.report["started_again"] = json!(again);
    for _ in lines.iter() {
        if deployment
            .documents("completed")
            .iter()
            .any(|job| job["job_id"] == json!(job_id))
        {
            break;
        }
    }
    let completed = deployment
        .documents("completed")
        .into_iter()
        .find(|job| job["job_id"] == json!(job_id))
        .expect("the job completes");
    deployment.report["completed_job"] = completed;
    deployment.pass();
}
