//! Real workload placement and execution on the machine running this test.
//!
//! Each case drives the built Stado CLI against an isolated local store whose
//! registry names this machine as a `local` target, so the production placement
//! path claims the submitted job for the current host and the operating system
//! really executes the shell command. The evidence is state the workload
//! process itself left behind — the artifact it wrote into its own working
//! directory and which the product then uploaded, the persisted queue record,
//! and the persisted status document carrying the real exit status. Stdout is
//! read only as supporting detail.
//!
//! See [`harness`] for the isolation contract. No provider, no cloud, no
//! simulated executor: a leg that would need one is not written here.

mod harness;

use std::fs;
use std::path::Path;

use serde_json::Value;

use harness::{said, Area};

/// The status a failing workload is asked for, so the recorded exit code can
/// only have come from the process that really ran.
const DELIBERATE_EXIT: i32 = 17;
const STDERR_MARKER: &str = "stado-workload-stderr-marker";
const STDOUT_MARKER: &str = "workload-stdout-marker";
/// A canonical job id (`job-` + 24 lowercase hex) that was never placed.
const NEVER_PLACED: &str = "job-000000000000000000000000";

#[test]
fn a_placed_workload_runs_here_and_its_record_carries_what_the_process_did() {
    let area = Area::new();
    let job = area.submit(
        "workload-real-success",
        &format!(
            "printf '%s\\n' \"$(hostname)\" > output/receipt.txt; \
             printf 'pwd=%s\\n' \"$PWD\" >> output/receipt.txt; echo {STDOUT_MARKER}"
        ),
    );
    area.drain();

    // What the executed process itself wrote, uploaded by the product.
    let receipt = area.read(&format!("status/{job}/output/receipt.txt"));
    let mut lines = receipt.lines();
    assert_eq!(
        lines.next(),
        Some(area.hostname.as_str()),
        "the workload did not run on this host: {receipt}"
    );
    let workdir = lines
        .next()
        .and_then(|line| line.strip_prefix("pwd="))
        .unwrap_or_else(|| panic!("the workload recorded no working directory: {receipt}"));
    assert!(
        area.is_own_workdir(workdir, &job),
        "the workload ran outside its isolated working directory: {workdir}"
    );

    // The persisted result of that run.
    assert_eq!(area.read(&format!("status/{job}/status")), "COMPLETED");
    assert_eq!(
        area.read(&format!("status/{job}/output/command_output.log")),
        format!("{STDOUT_MARKER}\n")
    );
    let record = area.record("completed", &job);
    assert_eq!(record["state"], "completed");
    assert_eq!(record["error"], Value::Null);
    assert_eq!(
        record["instance_ref"],
        Value::from(format!("local@{}", area.hostname))
    );
    assert!(record["started_at"].is_string() && record["completed_at"].is_string());
    let drained = area.record("queue", &job);
    assert!(
        drained["state"]
            .as_str()
            .is_some_and(|state| state.starts_with("transition-cleaned:")),
        "the finished workload is still claimable from the queue: {}",
        drained["state"]
    );

    // The product's own status verb reads that same terminal result.
    let watched = area.stado(&["job", "watch", &job, "--follow", "--json"]);
    assert!(
        watched.status.success(),
        "watch failed: {}",
        said(&watched.stderr)
    );
    let seen: Value = serde_json::from_str(&said(&watched.stdout)).expect("watch emits JSON");
    assert_eq!(seen["job"]["state"], "completed");
    assert_eq!(seen["terminal"], true);
    assert_eq!(seen["log"], Value::from(format!("{STDOUT_MARKER}\n")));

    // And its own download verb hands the artifact back byte for byte.
    let downloaded = area.scratch("results");
    let results = area.stado(&["results", &job, downloaded.to_str().unwrap()]);
    assert!(
        results.status.success(),
        "results failed: {}",
        said(&results.stderr)
    );
    assert_eq!(
        fs::read_to_string(downloaded.join("receipt.txt")).unwrap(),
        receipt
    );
}

#[test]
fn a_workload_that_exits_nonzero_records_its_real_status_and_last_words() {
    let area = Area::new();
    let job = area.submit(
        "workload-real-failure",
        &format!(
            "printf 'ran-before-failing\\n' > output/receipt.txt; \
             echo {STDERR_MARKER} 1>&2; exit {DELIBERATE_EXIT}"
        ),
    );
    area.drain();

    assert_eq!(
        area.read(&format!("status/{job}/output/receipt.txt")),
        "ran-before-failing\n",
        "the failing workload never really ran"
    );
    assert_eq!(
        area.read(&format!("status/{job}/status")),
        format!("FAILED exit={DELIBERATE_EXIT}"),
        "the persisted status lost the process's real exit code"
    );
    let record = area.record("failed", &job);
    assert_eq!(record["state"], "failed");
    assert!(record["failed_at"].is_string());
    assert_eq!(
        record["error"],
        Value::from(format!("workload exited unsuccessfully: {STDERR_MARKER}")),
        "the recorded failure is generic instead of the workload's own words"
    );
    assert!(
        !area.holds(&format!("completed/{job}.json")),
        "a failed workload was also recorded as completed"
    );

    let watched = area.stado(&["job", "watch", &job, "--follow"]);
    assert_eq!(watched.status.code(), Some(1));
    let refusal =
        format!("job {job} ended failed: workload exited unsuccessfully: {STDERR_MARKER}");
    assert!(
        said(&watched.stderr).contains(&refusal),
        "expected {refusal:?}, got: {}",
        said(&watched.stderr)
    );
}

#[test]
fn placing_a_workload_on_a_host_the_registry_does_not_declare_is_refused() {
    let area = Area::new();
    let before = area.read("registry.json");
    let output = area.stado(&["agent", "--target", "no-such-workload-host"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        said(&output.stderr).contains("target 'no-such-workload-host' not found in registry"),
        "got: {}",
        said(&output.stderr)
    );
    assert_eq!(
        area.read("registry.json"),
        before,
        "the fleet was rewritten"
    );
    assert!(
        !area.holds("capacity"),
        "a refused placement still published capacity"
    );
}

#[test]
fn a_workload_that_names_no_command_is_refused_before_anything_is_queued() {
    let area = Area::new();
    let output = area.stado(&["submit", "--run-id", "workload-real-no-command", "   "]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        said(&output.stderr).contains("command cannot be empty"),
        "got: {}",
        said(&output.stderr)
    );
    for prefix in ["queue", "queue_priority", "runs"] {
        assert!(
            !area.holds(prefix),
            "a refused workload left {prefix}/ behind"
        );
    }
    assert!(
        Path::new(&area.home).read_dir().unwrap().next().is_none(),
        "a refused workload created a job working directory"
    );
}

#[test]
fn watching_a_workload_that_was_never_placed_is_refused() {
    let area = Area::new();
    let output = area.stado(&["job", "watch", NEVER_PLACED]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        said(&output.stderr).contains(&format!("NOT_FOUND: job '{NEVER_PLACED}' was not found")),
        "got: {}",
        said(&output.stderr)
    );
}
