//! `stado overview --json` states the job outcomes the Stado Desktop fleet
//! screens read: per-model counts taken from the job commands, every failed
//! and completed job, and the measured throughput. Driven only through the
//! built binary against an isolated local store (`fixture::Store`), with
//! every command, exit status and the document kept in the case's report.

use serde_json::Value;

use crate::fixture::Store;

#[test]
fn overview_counts_queued_jobs_per_named_model_and_states_unmeasured_throughput() {
    let mut store = Store::new("overview-outcomes");
    let named = [
        "python run.py --model alpha --task first",
        "python run.py --model alpha --task second",
    ];
    let unnamed = ["python run.py"];
    for command in named.iter().chain(unnamed.iter()) {
        store.submit("overview-outcomes-run", command);
    }

    let document: Value = serde_json::from_str(&store.succeeded(&["overview", "--json"]))
        .expect("overview --json prints one JSON document");
    let outcomes = document["outcomes"].clone();
    store.observe("jobs", document["jobs"].clone());
    store.observe("outcomes", outcomes.clone());

    assert_eq!(
        document["jobs"]["queue"].as_u64(),
        Some((named.len() + unnamed.len()) as u64),
        "every submitted job is queued: {document}"
    );
    let by_model = outcomes["by_model"]
        .as_array()
        .expect("outcomes.by_model is a list");
    let alpha = by_model
        .iter()
        .find(|row| row["model"] == "alpha")
        .expect("the model the commands name is counted under its name");
    assert_eq!(alpha["queue"].as_u64(), Some(named.len() as u64), "{alpha}");
    // A command that names no model is counted under null, not under a
    // made-up model name.
    let none = by_model
        .iter()
        .find(|row| row["model"].is_null())
        .expect("commands naming no model are counted under null");
    assert_eq!(none["queue"].as_u64(), Some(unnamed.len() as u64), "{none}");

    // Nothing has run: no failed or completed job is listed, and the
    // throughput says it measured nothing instead of zero seconds.
    assert!(
        outcomes["failed"]
            .as_array()
            .expect("outcomes.failed is a list")
            .is_empty(),
        "{outcomes}"
    );
    let completed = outcomes["completed"]
        .as_array()
        .expect("outcomes.completed is a list");
    assert!(completed.is_empty(), "{outcomes}");
    assert!(
        outcomes["throughput"]["average_wall_seconds_per_completed_job"].is_null(),
        "an average over no measured job is null: {outcomes}"
    );
    assert_eq!(
        outcomes["throughput"]["samples"].as_u64(),
        Some(completed.len() as u64),
        "{outcomes}"
    );
    store.pass();
}
