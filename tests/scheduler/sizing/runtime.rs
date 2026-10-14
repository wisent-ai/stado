//! A positive priority must not invent a finish time. Real worker publication,
//! CLI submissions and a real coordinator tick over an isolated local store.
//! The worker is stopped before submitting work; its unexpired publication is
//! retained as real measured state, not replaced with a synthetic capacity row.
use crate::deployment::{line_with, line_with_any, Deployment};
use serde_json::{json, Value};

fn required(name: &str) -> Result<String, Box<dyn std::error::Error>> {
    std::env::var(name)
        .map_err(|cause| format!("{name} is required in the journey environment: {cause}").into())
}

fn queued(deployment: &Deployment, run: &str) -> Result<Value, Box<dyn std::error::Error>> {
    deployment
        .documents("queue")
        .into_iter()
        .find(|job| job["run_id"].as_str() == Some(run))
        .ok_or_else(|| format!("submitted run {run} is missing from the isolated queue").into())
}

#[test]
fn priority_without_runtime_stays_unassigned_while_a_declared_runtime_is_used(
) -> Result<(), Box<dyn std::error::Error>> {
    let revision = required("STADO_SOURCE_REVISION")?;
    let poll = required("STADO_TEST_WORKER_POLL_SECONDS")?;
    let tick = required("STADO_TEST_COORDINATOR_INTERVAL_SECONDS")?;
    let priority = required("STADO_TEST_POSITIVE_PRIORITY")?;
    let runtime = required("STADO_TEST_RUNTIME_SECONDS")?;
    assert!(priority.parse::<i64>()?.is_positive());
    let mut deployment = Deployment::start("runtime-priority");
    let version = deployment.cli(&["--version"]);
    assert!(
        version.contains(&revision),
        "binary does not identify {revision}: {version}"
    );
    let lines = deployment.serve(&[
        "--worker",
        "--standalone",
        "--kind",
        "local",
        "--poll-seconds",
        &poll,
    ]);
    let publication = line_with_any(
        &lines,
        &[
            "published accepting_jobs=true",
            "published accepting_jobs=false",
            "ordinary work remains blocked",
            "capacity publish REFUSED",
        ],
    );
    deployment.report["publication_log"] = json!(publication);
    deployment.report["capacity"] = json!(deployment.documents("capacity"));
    deployment.save();
    assert!(
        publication.contains("published accepting_jobs=true"),
        "the real worker cannot admit this journey: {publication}"
    );
    deployment.stop();

    deployment.cli(&[
        "submit",
        "--run-id",
        "runtime-unknown",
        "--provider",
        "local",
        "--pin-provider",
        "true",
    ]);
    deployment.cli(&[
        "submit",
        "--run-id",
        "runtime-priority",
        "--provider",
        "local",
        "--pin-provider",
        "--priority",
        &priority,
        "true",
    ]);
    deployment.cli(&[
        "submit",
        "--run-id",
        "runtime-declared",
        "--provider",
        "local",
        "--pin-provider",
        "--priority",
        &priority,
        "--runtime-seconds-estimate",
        &runtime,
        "true",
    ]);
    deployment.report["queue_before_tick"] = json!(deployment.documents("queue"));
    deployment.save();
    let lines = deployment.serve(&[
        "--control-plane",
        "local",
        "--control-plane-interval-seconds",
        &tick,
    ]);
    deployment.report["tick"] = json!(line_with(&lines, "tick scheduled="));
    let declared = queued(&deployment, "runtime-declared")?;
    let ordinary = queued(&deployment, "runtime-unknown")?;
    let prioritized = queued(&deployment, "runtime-priority")?;
    deployment.report["queue_after_tick"] = json!([&declared, &ordinary, &prioritized]);
    deployment.save();
    let consumer = declared["assigned_to"]
        .as_str()
        .expect("assignment is text");
    assert!(!consumer.is_empty(),
        "declared runtime must use the real capacity; an expired or unusable publication is not a pass: {declared}");
    let capacities = deployment.report["capacity"]
        .as_array()
        .expect("capacity records");
    assert!(
        capacities
            .iter()
            .any(|row| row["consumer_id"].as_str() == Some(consumer)),
        "the assignment must name the worker that really published capacity"
    );
    for job in [&ordinary, &prioritized] {
        assert_eq!(
            job["assigned_to"].as_str(),
            Some(""),
            "priority cannot provide a missing runtime: {job}"
        );
    }
    deployment.pass();
    Ok(())
}
