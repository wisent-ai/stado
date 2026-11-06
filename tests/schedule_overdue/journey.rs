//! `schedule list` names a schedule that did not fire past its next run.
//!
//! One isolated deployment with a local store no coordinator sweeps:
//! `schedule create` declares an every-minute schedule, and once its first
//! run time passes, read from the store through `schedule list` itself,
//! `schedule list --json` carries `overdue` on it saying no sweep was ever
//! recorded, the same reason `schedule show` gives, and the text listing
//! prints that reason on a line under its row. Before the run time it
//! carries none.
use crate::deployment::Deployment;
use serde_json::Value;

/// The one schedule `schedule list --json` answers.
fn listed(deployment: &mut Deployment) -> Value {
    let listed: Value = serde_json::from_str(&deployment.cli(&["schedule", "list", "--json"])).unwrap();
    listed
        .as_array()
        .and_then(|schedules| schedules.first())
        .cloned()
        .unwrap_or_else(|| panic!("the store lists the declared schedule: {listed}"))
}

#[test]
fn a_schedule_past_its_run_with_no_sweep_is_listed_overdue() {
    let mut deployment = Deployment::start("overdue-listed");
    let store = deployment.store();
    deployment.set("WC_STORAGE_BACKEND", "local");
    deployment.set("WC_LOCAL_STORAGE_PATH", store.to_str().expect("the run directory is UTF-8"));
    deployment.cli(&["schedule", "create", "--cron", "* * * * *", "echo overdue-listed"]);

    let before = listed(&mut deployment);
    let due = chrono::DateTime::parse_from_rfc3339(before["next_due_at"].as_str().expect("the schedule has a next run"))
        .expect("next_due_at is an ISO-8601 time")
        .with_timezone(&chrono::Utc);
    let remaining = due - chrono::Utc::now();
    if remaining > chrono::TimeDelta::zero() {
        assert!(before.get("overdue").is_none(), "a schedule not yet due carries overdue: {before}");
        // The schedule's own run time is what must pass, and only the
        // machine's clock passes it: nothing in the product announces it.
        std::thread::sleep(remaining.to_std().expect("a positive span"));
    }

    let after = listed(&mut deployment);
    deployment.report["listed"] = after.clone();
    let id = after["schedule_id"].as_str().expect("the schedule has an id").to_string();
    let reason = after["overdue"].as_str().unwrap_or_else(|| panic!("a schedule past its run lists no overdue: {after}"));
    assert!(reason.contains("no coordinator has recorded a schedule sweep"), "{reason}");
    let shown: Value = serde_json::from_str(&deployment.cli(&["schedule", "show", &id, "--json"])).unwrap();
    assert_eq!(shown["overdue"].as_str(), Some(reason), "list and show give one reason: {shown}");

    let table = deployment.cli(&["schedule", "list"]);
    assert!(table.contains(&format!("  overdue: {reason}")), "the table names the reason under the row: {table}");
    deployment.report["outcome"] = "passed".into();
    deployment.save();
}
