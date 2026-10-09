//! Real `stado status` reads against the declared qualification
//! configuration: a whole job id whose run the reaper has settled is found
//! in the run's retained outcome and printed with its terminal state, and a
//! whole id nothing holds is refused by name. Nothing is seeded. The same
//! story on an isolated local queue, with the start of an id and `--json`,
//! is `by_id.rs`.
// The desktop API fixture is shared: this journey drives the CLI only, so the
// fixture's HTTP client and its `call`/`persisted` helpers go unused here.
#[allow(dead_code)]
#[path = "../desktop_api/fixture.rs"]
mod fixture;
use fixture::Service;
use std::path::Path;

/// One command line as the operator types it, split into its arguments.
fn argv(line: &str) -> Vec<&str> {
    line.split_whitespace().collect()
}

#[tokio::test]
async fn a_reaped_job_is_read_back_by_its_whole_id() {
    let mut service = Service::start_with_configuration("STADO_STATUS_QUALIFICATION_CONFIG");
    let id = service.input("STADO_STATUS_REAPED_JOB_ID");
    let line = format!("status {id}");
    let printed = service.cli(&argv(&line));
    let row = printed.lines().find(|row| row.starts_with(id.as_str()));
    assert!(row.is_some(), "no row for {id}: {printed}");
    let row = row.into_iter().collect::<String>();
    assert!(
        row.contains("(reaped)"),
        "the row names the retained outcome: {row}"
    );
    service.observe("row", serde_json::json!(row));

    // The same hex read backwards is a whole id of the same form that no
    // job holds.
    let hex: String = id.trim_start_matches("job-").chars().rev().collect();
    let unknown = format!("job-{hex}");
    let line = format!("status {unknown}");
    let output = service.execute(Path::new(env!("CARGO_BIN_EXE_stado")), &argv(&line));
    assert!(!output.status.success(), "an unknown id is a refusal");
    let said = format!(
        "no job with id {unknown} in the queue or in any run's retained outcomes; `stado \
         status` lists the jobs the queue holds"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(&said), "{stderr}");
    service.pass();
}
