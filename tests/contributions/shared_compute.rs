//! `stado contributions` over jobs this journey runs itself.
//!
//! One isolated deployment: two jobs are submitted and the real `stado serve`
//! worker runs both on this host until each is in `completed/` — one that
//! sleeps, so its record measures whole seconds of work, and one that exits
//! at once, so its record measures none. While the registry declares no
//! `contribution_owner` for the host, `pending` credits nobody: the sleeping
//! job is unmeasured with that reason. Once the deployment's registry declares
//! an owner for the host, `pending` must answer exactly those two: the first
//! as the only `compute.shared` event of the two, of that owner, with the
//! seconds between its recorded start and completion and its completion as
//! its time; the second as unmeasured with the reason. The owner comes from
//! the registry, so nobody claims a host by a name they set: `link` is no
//! verb, and is refused as one. `report` with no ledger named is refused by
//! the setting's name.
//!
//! Given STADO_CONTRIBUTION_TEST_LEDGER (a qualification ledger the run owns,
//! never the fleet's own) and STADO_CONTRIBUTION_TEST_BEARER (the bearer its
//! operator declared for the reporter `stado`), the bearer is stored in the
//! deployment's own credential store and the ledger leg runs: `pending`
//! marks the event not held, `report` records it, a second `report` sends
//! nothing and names it held, and `pending` then marks it held. Without them
//! the report says the leg was not run and why.
use crate::deployment::Deployment;
use serde_json::{json, Value};

const LEDGER_ENV: &str = "STADO_CONTRIBUTION_TEST_LEDGER";
const BEARER_ENV: &str = "STADO_CONTRIBUTION_TEST_BEARER";
const MEASURED: &str = "contributions-measured";
const INSTANT: &str = "contributions-instant";

/// Whole seconds between a record's start and completion.
fn seconds(job: &Value) -> i64 {
    let time = |field: &str| {
        let Some(text) = job[field].as_str() else {
            panic!("the completed record states no {field}: {job}");
        };
        match chrono::DateTime::parse_from_rfc3339(text) {
            Ok(time) => time,
            Err(error) => panic!("{field} {text:?} is not an RFC 3339 time: {error}"),
        }
    };
    (time("completed_at") - time("started_at")).num_seconds()
}

fn json_of(text: &str) -> Value {
    match serde_json::from_str(text) {
        Ok(value) => value,
        Err(error) => panic!("the command did not print JSON ({error}): {text}"),
    }
}

/// The entry of `rows` whose `key` is `id`.
fn row<'a>(rows: &'a Value, key: &str, id: &Value) -> &'a Value {
    match rows
        .as_array()
        .and_then(|rows| rows.iter().find(|row| &row[key] == id))
    {
        Some(row) => row,
        None => panic!("{id} is not among {rows}"),
    }
}

#[test]
fn completed_jobs_are_reported_as_the_compute_their_hosts_shared() {
    let mut deployment = Deployment::start("contributions");
    deployment.cli(&["submit", "--run-id", MEASURED, "sleep 2"]);
    deployment.cli(&["submit", "--run-id", INSTANT, "true"]);
    let runs = [MEASURED, INSTANT];
    let lines = deployment.serve(&["--worker", "--kind", "local", "--poll-seconds", "1"]);
    let finished = |deployment: &Deployment| {
        let done: Vec<Value> = deployment
            .documents("completed")
            .into_iter()
            .filter(|job| runs.iter().any(|run| job["run_id"] == json!(run)))
            .collect();
        (done.len() == runs.len()).then_some(done)
    };
    let mut completed = None;
    for line in lines.iter() {
        completed = finished(&deployment);
        if completed.is_some() {
            break;
        }
        deployment.report["last_service_line"] = json!(line);
    }
    let completed =
        completed.expect("the service ended before both jobs completed; inspect service.stderr");
    deployment.report["completed"] = json!(completed);
    let job = |run: &str| {
        completed
            .iter()
            .find(|job| job["run_id"] == json!(run))
            .expect("each run completed")
            .clone()
    };
    let (measured, instant) = (job(MEASURED), job(INSTANT));
    assert!(
        seconds(&measured).is_positive(),
        "the sleeping job measured no whole second: {measured}"
    );
    assert!(
        !seconds(&instant).is_positive(),
        "the job that exits at once took whole seconds on this host, so the unmeasured case cannot be shown: {instant}"
    );

    // No owner declared: nobody is credited, and the reason says so.
    let host = measured["worker_allocation"]["host"]
        .as_str()
        .expect("the measured job names its host")
        .to_string();
    let unowned = json_of(&deployment.cli(&["contributions", "pending", "--json"]));
    let reason = &row(&unowned["unmeasured"], "job", &measured["job_id"])["reason"];
    assert!(
        reason
            .as_str()
            .is_some_and(|reason| reason.contains("registry")),
        "{unowned}"
    );

    // The deployment's own registry declares the host and its owner.
    let (listed, _, _) = deployment.run(&["registry", "host", "show", &host], "");
    if !listed {
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
    let owner = "owner@example.test";
    let path = format!("targets.{host}.contribution_owner");
    deployment.cli(&["registry", "set", "--path", &path, "--value", owner]);

    let pending = json_of(&deployment.cli(&["contributions", "pending", "--json"]));
    let ours: Vec<&Value> = pending["events"]
        .as_array()
        .expect("pending lists events")
        .iter()
        .map(|event| &event["source_event_id"])
        .filter(|id| **id == measured["job_id"] || **id == instant["job_id"])
        .collect();
    assert_eq!(
        ours,
        vec![&measured["job_id"]],
        "only the measured job is an event: {pending}"
    );
    let event = row(&pending["events"], "source_event_id", &measured["job_id"]);
    assert_eq!(event["kind"], json!("compute.shared"));
    assert_eq!(event["unit"], json!("second"));
    assert_eq!(event["quantity"], json!(seconds(&measured)));
    assert_eq!(event["contributor_id"], json!(owner));
    assert_eq!(event["evidence"]["host"], json!(host));
    assert_eq!(event["occurred_at"], measured["completed_at"]);
    assert!(
        event.get("held").is_none(),
        "pending asked a ledger nobody named: {event}"
    );
    let unmeasured = row(&pending["unmeasured"], "job", &instant["job_id"]);
    assert!(
        unmeasured["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("measures no work")),
        "{unmeasured}"
    );

    let (reported, _, refusal) = deployment.run(&["contributions", "report"], "");
    assert!(
        !reported && refusal.contains("STADO_CONTRIBUTION_LEDGER_URL"),
        "report without a ledger: {refusal}"
    );
    let (linked, _, refusal) = deployment.run(&["contributions", "link", "unspent"], "");
    assert!(
        !linked && refusal.contains("link"),
        "link is no verb: {refusal}"
    );

    match (std::env::var(LEDGER_ENV), std::env::var(BEARER_ENV)) {
        (Ok(ledger), Ok(bearer)) => {
            ledger_leg(&mut deployment, &ledger, &bearer, &measured["job_id"])
        }
        (ledger, bearer) => {
            deployment.report["ledger_leg"] = json!(format!(
                "not run: {LEDGER_ENV} {ledger:?}; {BEARER_ENV} set: {}",
                bearer.is_ok()
            ));
        }
    }
    deployment.pass();
}

fn ledger_leg(deployment: &mut Deployment, ledger: &str, bearer: &str, job: &Value) {
    let token = json!({"token": bearer}).to_string();
    let (stored, _, refusal) =
        deployment.run(&["credentials", "put", "stado-contribution-ledger"], &token);
    assert!(stored, "the bearer could not be stored: {refusal}");
    let pending = |deployment: &mut Deployment| {
        json_of(&deployment.cli(&["contributions", "pending", "--ledger", ledger, "--json"]))
    };
    assert_eq!(
        row(&pending(deployment)["events"], "source_event_id", job)["held"],
        json!(false)
    );
    let first =
        json_of(&deployment.cli(&["contributions", "report", "--ledger", ledger, "--json"]));
    assert_eq!(
        row(&first["events"], "job", job)["answer"]["recorded"],
        json!(true),
        "{first}"
    );
    let second =
        json_of(&deployment.cli(&["contributions", "report", "--ledger", ledger, "--json"]));
    assert_eq!(
        second["events"],
        json!([]),
        "a held event was sent again: {second}"
    );
    assert!(
        second["held"]
            .as_array()
            .is_some_and(|held| held.contains(job)),
        "{second}"
    );
    assert_eq!(
        row(&pending(deployment)["events"], "source_event_id", job)["held"],
        json!(true)
    );
    deployment.report["ledger_leg"] = json!("passed");
}
