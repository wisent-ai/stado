//! `stado status <id>` on a real local queue: a job is submitted, cancelled
//! and reaped through the built `stado`, and is then read back by its whole
//! id and by the first characters of its id, as text and as `--json`, from
//! the outcome its run retained; an id no job holds is refused by name. The
//! store is inspected after each step: the job's own document is gone once
//! reaped and the index names its run.
#[path = "fixture.rs"]
mod fixture;
use fixture::Store;
use serde_json::{json, Value};

/// The run every job of this case is submitted under; its length is also
/// how many characters of a job id's hex the case types as the id's start.
const RUN_ID: &str = "status-by-id";

/// The one row of a listing that names `job_id`.
fn row<'a>(printed: &'a str, job_id: &str) -> &'a str {
    match printed.lines().find(|line| line.starts_with(job_id)) {
        Some(line) => line,
        None => panic!("no row for {job_id}:\n{printed}"),
    }
}

/// The hex of a job id.
fn hex(job_id: &str) -> &str {
    job_id.strip_prefix("job-").expect("a job id starts with job-")
}

#[test]
fn a_reaped_job_is_read_by_its_whole_id_and_by_its_start_and_an_unknown_id_is_refused() {
    let mut store = Store::new(RUN_ID);
    let job_id = store.submit(RUN_ID, "echo status");
    let jobs = [job_id.as_str()];
    store.cancel(&job_id);
    assert!(
        row(&store.ok(&["status", &job_id]), &job_id).contains("cancelled"),
        "a cancelled job still in the queue reads as cancelled"
    );

    store.reap(jobs.len(), "1");
    assert!(
        !store.store().join(format!("cancelled/{job_id}.json")).is_file(),
        "the reaper deletes the job's own document"
    );
    let index = store.store().join(format!("runs/jobs/{job_id}"));
    assert_eq!(
        std::fs::read_to_string(&index).expect("the reaper indexes the job by its run").trim(),
        RUN_ID,
        "the index names the run that retained the job"
    );

    // Whole id, as text.
    let printed = store.ok(&["status", &job_id]);
    let whole = row(&printed, &job_id).to_string();
    assert!(whole.contains("cancelled (reaped)"), "{whole}");
    store.observe("whole_id_row", json!(whole));

    // The start of the id, with and without `job-`.
    let start = &hex(&job_id)[..RUN_ID.len()];
    let printed = store.ok(&["status", start]);
    assert_eq!(row(&printed, &job_id), whole, "{printed}");
    let printed = store.ok(&["status", &format!("job-{start}")]);
    assert_eq!(row(&printed, &job_id), whole, "{printed}");

    // The same rows as JSON, with the lifecycle state and the reaped mark.
    let rows: Value = serde_json::from_str(&store.ok(&["status", &job_id, "--json"]))
        .expect("--json prints JSON");
    let listed = rows.as_array().expect("--json prints an array");
    let first = listed.first().expect("the job's row");
    assert_eq!(first["job_id"], job_id, "{rows}");
    assert_eq!(first["state"], "cancelled", "{rows}");
    assert_eq!(first["reaped"], true, "{rows}");
    assert_eq!(listed.len(), jobs.len(), "{rows}");
    let by_start: Value = serde_json::from_str(&store.ok(&["status", start, "--json"]))
        .expect("--json prints JSON");
    assert_eq!(by_start, rows);
    store.observe("json", rows);

    // An id no job holds: the same hex read backwards is a whole id of the
    // same form, and its start is a start no job has.
    let reversed: String = hex(&job_id).chars().rev().collect();
    for unknown in [format!("job-{reversed}"), reversed[..RUN_ID.len()].to_string()] {
        let output = store.run(&["status", &unknown]);
        assert!(!output.status.success(), "an unknown id is a refusal");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(&format!(
                "no job with id {unknown} in the queue or in any run's retained outcomes; \
                 `stado status` lists the jobs the queue holds"
            )),
            "{stderr}"
        );
        let output = store.run(&["status", &unknown, "--json"]);
        assert!(!output.status.success(), "an unknown id is a refusal with --json too");
        let said = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(said.contains(&format!("no job with id {unknown}")), "{said}");
    }
    store.pass();
}
