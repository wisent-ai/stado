//! `stado host activity` over jobs this journey runs itself.
//!
//! One isolated deployment: two jobs are submitted, one that succeeds and one
//! that fails, and the real `stado serve` worker runs both on this host until
//! one is under `completed/` and the other under `failed/`. Once the
//! deployment's registry declares the host, `stado host activity <host>
//! --json` must list those two among its jobs, each with the lifecycle of the
//! prefix its record is under, the command it ran and the worker kind that
//! ran it, oldest first; `--since` a time read after both ended lists
//! neither; a `--since` that is not an RFC 3339 time is refused naming the
//! form, and a host the registry does not declare is refused. The text form
//! names both jobs.
use crate::deployment::Deployment;
use serde_json::{json, Value};

const SUCCEEDS: &str = "host-activity-succeeds";
const FAILS: &str = "host-activity-fails";

fn json_of(text: &str) -> Value {
    match serde_json::from_str(text) {
        Ok(value) => value,
        Err(error) => panic!("the command did not print JSON ({error}): {text}"),
    }
}

/// The record of the run `run` under `prefix`, once the worker put it there.
fn record(deployment: &Deployment, prefix: &str, run: &str) -> Option<Value> {
    deployment
        .documents(prefix)
        .into_iter()
        .find(|job| job["run_id"] == json!(run))
}

/// The entry of `activity` for the job `job` records.
fn entry<'a>(activity: &'a Value, job: &Value) -> &'a Value {
    let found = activity["jobs"]
        .as_array()
        .and_then(|jobs| jobs.iter().find(|entry| entry["job"] == job["job_id"]));
    match found {
        Some(entry) => entry,
        None => panic!("{} is not in the activity: {activity}", job["job_id"]),
    }
}

#[test]
fn a_host_activity_lists_the_jobs_its_worker_ran() {
    let mut deployment = Deployment::start("host-activity");
    deployment.cli(&["submit", "--run-id", SUCCEEDS, "true"]);
    deployment.cli(&["submit", "--run-id", FAILS, "false"]);
    let lines = deployment.serve(&["--worker", "--kind", "local", "--poll-seconds", "1"]);
    let mut ended = None;
    for line in lines.iter() {
        ended = match (
            record(&deployment, "completed", SUCCEEDS),
            record(&deployment, "failed", FAILS),
        ) {
            (Some(succeeded), Some(failed)) => Some((succeeded, failed)),
            _ => None,
        };
        if ended.is_some() {
            break;
        }
        deployment.report["last_service_line"] = json!(line);
    }
    let (succeeded, failed) =
        ended.expect("the service ended before both jobs ended; inspect service.stderr");
    deployment.stop();
    deployment.report["ended"] = json!([succeeded, failed]);
    // Read once both records are written, so every end they state is before it.
    let after_both = chrono::Utc::now().to_rfc3339();

    let host = succeeded["worker_allocation"]["host"]
        .as_str()
        .expect("the completed job names the host that ran it")
        .to_string();
    let (listed, _, _) = deployment.run(&["registry", "host", "show", &host], "");
    if !listed {
        let (answered, _, refusal) = deployment.run(&["host", "activity", &host, "--json"], "");
        deployment.report["undeclared_refusal"] = json!(refusal);
        assert!(
            !answered,
            "a host the registry does not declare was answered as one"
        );
        let platform = if cfg!(target_os = "macos") {
            "darwin-arm64"
        } else {
            "linux-amd64"
        };
        let ssh = format!("stado@{host}");
        deployment.cli(&[
            "registry",
            "host",
            "add",
            &host,
            "--ssh",
            &ssh,
            "--release-platform",
            platform,
        ]);
    }

    let activity = json_of(&deployment.cli(&["host", "activity", &host, "--json"]));
    let (done, broken) = (entry(&activity, &succeeded), entry(&activity, &failed));
    assert_eq!(done["lifecycle"], json!("completed"), "{activity}");
    assert_eq!(broken["lifecycle"], json!("failed"), "{activity}");
    assert_eq!(done["command"], succeeded["command"], "{activity}");
    assert_eq!(broken["command"], failed["command"], "{activity}");
    assert_eq!(done["kind"], json!("local"), "{activity}");
    assert_eq!(
        done["completed_at"], succeeded["completed_at"],
        "{activity}"
    );
    assert_eq!(broken["failed_at"], failed["failed_at"], "{activity}");
    let times: Vec<&Value> = activity["jobs"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|job| &job["at"])
        .collect();
    let mut ordered = times.clone();
    ordered.sort_by_key(|at| at.as_str().map(str::to_owned));
    assert_eq!(times, ordered, "the timeline is oldest first: {activity}");

    let later =
        json_of(&deployment.cli(&["host", "activity", &host, "--since", &after_both, "--json"]));
    assert!(
        later["jobs"].as_array().is_some_and(|jobs| !jobs
            .iter()
            .any(|job| job["job"] == succeeded["job_id"] || job["job"] == failed["job_id"])),
        "--since after both ended still lists them: {later}"
    );

    let (accepted, _, refusal) =
        deployment.run(&["host", "activity", &host, "--since", "yesterday"], "");
    assert!(
        !accepted && refusal.contains("RFC 3339"),
        "a --since that is no time: {refusal}"
    );

    let text = deployment.cli(&["host", "activity", &host]);
    for job in [&succeeded, &failed] {
        let id = job["job_id"].as_str().expect("a job record names its id");
        assert!(text.contains(id), "the text form omits {id}: {text}");
    }
    deployment.pass();
}
