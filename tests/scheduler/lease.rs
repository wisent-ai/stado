//! A running job carries the promise its worker made, and the reaper holds
//! it to exactly that: a job that runs across many renewals is never
//! requeued, and finishes where it started.
//!
//! One isolated deployment: `config init` seeds a local store, a CPU job
//! that runs for several of the worker's poll periods is submitted, and the
//! real `stado serve` runs both the local control plane (coordinator tick
//! with the lease reaper) and a worker polling every second. While the job
//! runs, its running document must state a `lease_expires_at` in the future
//! — the worker's promise — and the job must end completed with no restart
//! and no lease-expiry requeue in the log.
#[path = "deployment.rs"]
mod deployment;

use deployment::{line_with, line_with_any, Deployment};
use serde_json::json;

#[test]
fn a_running_job_is_held_to_its_workers_promise_and_completes() {
    let mut deployment = Deployment::start("lease");
    // Runs for several one-second poll periods without naming a model, so
    // any local worker may claim it.
    let command = "perl -e 'select(undef, undef, undef, 6)'";
    deployment.cli(&["submit", "--run-id", "lease-journey", command]);
    let lines = deployment.serve(&[
        "--control-plane",
        "local",
        "--control-plane-interval-seconds",
        "5",
        "--worker",
        "--kind",
        "local",
        "--poll-seconds",
        "1",
    ]);

    // A worker that will not take ordinary work says so on every loop
    // (`ordinary work remains blocked`): the journey cannot run here, and
    // that line is the reason, not a pass.
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
    let promise = chrono::DateTime::parse_from_rfc3339(
        running["lease_expires_at"].as_str().unwrap_or_default(),
    )
    .unwrap_or_else(|error| {
        panic!("a running job states its worker's promise ({error}): {running}")
    });
    deployment.report["running_job"] = running.clone();
    let started =
        chrono::DateTime::parse_from_rfc3339(running["started_at"].as_str().unwrap_or_default())
            .unwrap_or_else(|error| {
                panic!("a running job states when it started ({error}): {running}")
            });
    assert!(
        promise - started >= chrono::Duration::seconds(1),
        "the claim promises at least the worker's one-second poll period: {running}"
    );

    // The job ends in exactly one place; a lease-expiry requeue ends the
    // wait as a failure.
    let mut requeues = Vec::new();
    for line in lines.iter() {
        if line.contains("worker lease expired") {
            requeues.push(line.clone());
            break;
        }
        if !deployment.documents("completed").is_empty()
            || !deployment.documents("failed").is_empty()
        {
            break;
        }
    }
    deployment.report["lease_expiry_lines"] = json!(requeues);
    assert!(requeues.is_empty(), "no lease-expiry requeue: {requeues:?}");
    let completed = deployment
        .documents("completed")
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("the job completes: {:?}", deployment.documents("failed")));
    deployment.report["completed_job"] = completed.clone();
    assert_eq!(completed["restarts"], json!(0), "{completed}");

    // The cleaned sentinel the move left in running/ is retired by the next
    // coordinator tick: its job is settled, and no age is waited out.
    deployment.report["sentinel_retired"] =
        json!(line_with(&lines, "running/ settled sentinels retired="));
    assert!(
        deployment.documents("running").is_empty(),
        "running/ holds nothing once the settled sentinel is retired: {:?}",
        deployment.documents("running")
    );
    deployment.pass();
}
