//! The build/release pipeline heals its own queue, and how the product says
//! so.
//!
//! Every test drives the built `stado` binary (`CARGO_BIN_EXE_stado`) with
//! WC_STORAGE_BACKEND=local + WC_LOCAL_STORAGE_PATH=<TempDir>. STADO_CONFIG
//! points at a nonexistent path so the developer's real config can never
//! leak in, and the running jobs, the queue records, the run receipts and
//! the registry document all live and die inside the temp dir. Nothing here
//! reads the fleet's store or touches a host.
//!
//! What is under test is one `stado coordinator --once` tick as the
//! self-healing mechanism: a `running/` job whose worker lease has expired
//! is requeued exactly once with the reason stored on the record, a second
//! expiry fails it with that same reason, a fresh lease is never touched,
//! and an unreachable registry is a stated skip, not a crash.
//!
//! Every fixture is copied from the live incident it was written for, not
//! invented. On 2026-08-19/20 jobs `59ca672f` and `d44f9352` sat in
//! `running/` for hours with no live worker behind them — phantom capacity
//! the scheduler kept counting — because the fleet has no cloud provider
//! arm, so nothing ever reaped them. The job body below is the exact shape
//! `stado submit 'echo hi'` writes under the local backend (captured
//! 2026-08-20), and the asserted sentences are the tick's own words from
//! that seeded state, not paraphrases:
//!
//! ```text
//! [tick] 05cd436e: requeued (worker lease expired; lease silent for 97156s; restart 1/20)
//! [tick] lease-reaper: requeued 1 phantom job(s), failed 0 on second expiry, cleared 0 silent-worker assignment(s)
//! [tick] 05cd436e: FAILED (worker lease expired; lease silent for 97178s; second lease expiry)
//! [tick] build poll skipped: registry unreachable: no registry document at local:registry.json
//! ```

use std::path::Path;
use std::process::{Command, Output};

use serde_json::{json, Value};

/// The first phantom of the live incident.
const PHANTOM_JOB: &str = "59ca672f";
/// The second phantom, used where a distinct record keeps a test's story
/// separate from the first.
const SECOND_PHANTOM: &str = "d44f9352";
/// The reason the reaper stores on a requeued record and matches on the
/// second expiry, verbatim `queue::reaper::LEASE_EXPIRED_REASON`.
const LEASE_EXPIRED: &str = "worker lease expired";
/// How long the live phantoms' leases had been silent when the mechanism
/// was written: ~27 hours, kept as an offset from the test's own clock so
/// the fixture is the same on any day, and far past the 15-minute
/// `HEARTBEAT_STALE_MINUTES` TTL.
const SILENT_SECONDS: i64 = 97_156;

/// One coordinator tick against the temp store, exactly as an operator or a
/// launchd unit runs it. Env is cleared so the developer's real config,
/// credentials and store can never leak in.
fn tick(storage: &Path) -> Output {
    let home = storage.parent().unwrap();
    Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(["coordinator", "--once"])
        .env_clear()
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin")
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", storage)
        .env("STADO_CONFIG", home.join("nonexistent-config.json"))
        .output()
        .expect("stado coordinator --once runs")
}

/// stdout and stderr as one haystack: the tick logs to stdout, degradation
/// notices to stderr, and a sentence asserted here must be found wherever
/// the product actually says it.
fn transcript(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// A temp HOME with the store directory inside it. The store starts empty:
/// each test seeds only what its story needs.
fn fleet() -> (tempfile::TempDir, std::path::PathBuf) {
    let home = tempfile::tempdir().unwrap();
    let storage = home.path().join("store");
    std::fs::create_dir_all(&storage).unwrap();
    (home, storage)
}

fn write(storage: &Path, name: &str, body: &Value) {
    let path = storage.join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, serde_json::to_string_pretty(body).unwrap()).unwrap();
}

fn read(storage: &Path, name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(storage.join(name)).unwrap()).unwrap()
}

/// `seconds` before the moment this test runs, RFC-3339.
fn ago(seconds: i64) -> String {
    (chrono::Utc::now() - chrono::Duration::seconds(seconds)).to_rfc3339()
}

/// The full job record shape `stado submit` writes under the local backend,
/// captured live on 2026-08-20, moved to `running/` the way a claiming
/// worker leaves it. `restarts`/`error` carry each test's prior history.
fn running_job(
    storage: &Path,
    job_id: &str,
    started_seconds_ago: i64,
    restarts: i64,
    error: Option<&str>,
) {
    let body = json!({
        "job_id": job_id,
        "command": "bash inputs/run.sh",
        "gpu_mem_gb": 0,
        "gpu_type": "",
        "machine_type": "e2-standard-8",
        "provider": "",
        "batch_id": format!("batch-{job_id}"),
        "state": "running",
        "created_at": ago(started_seconds_ago + 60),
        "started_at": ago(started_seconds_ago),
        "completed_at": null,
        "failed_at": null,
        "instance_ref": null,
        "restarts": restarts,
        "max_restarts": 20,
        "last_restart": null,
        "image": "pytorch-2-9-cu129-ubuntu-2204-nvidia-580-v20260408",
        "image_project": "deeplearning-platform-release",
        "boot_disk_gb": 500,
        "startup_script_uri": "",
        "error": error,
        "preemptible": false,
        "submitted_by": "lukaszbartoszcze",
        "submitted_from": "Lukaszs-MacBook-Pro-5485.local",
    });
    write(storage, &format!("running/{job_id}.json"), &body);
    // The listing metadata mirror the local backend keeps beside every
    // queue blob, in the exact shape `stado submit` created it.
    write(
        storage,
        &format!(".metadata/running/{job_id}.json"),
        &json!({"gpu_mem_gb": "0", "pin_to_provider": "false", "priority": "0"}),
    );
}

/// The batch receipt `stado submit` writes to `runs/`; the run-reaper folds
/// a terminal job's outcome into its `final_counts`, which is the durable
/// evidence once the per-job blob is cleaned up.
fn run_receipt(storage: &Path, job_id: &str) {
    write(
        storage,
        &format!("runs/run-{job_id}.json"),
        &json!({
            "run_id": format!("run-{job_id}"),
            "name": "1jobs",
            "created_at": ago(SILENT_SECONDS + 120),
            "submitter_app": "manual",
            "submitted_by": "lukaszbartoszcze",
            "submitted_from": "Lukaszs-MacBook-Pro-5485.local",
            "n_jobs": 1,
            "job_ids": [job_id],
            "commands": ["bash inputs/run.sh"],
        }),
    );
}

#[test]
fn first_lease_expiry_requeues_the_phantom_job_with_the_reason_stored() {
    let (_home, storage) = fleet();
    // A worker died before its first heartbeat: no `status/<id>/heartbeat`
    // blob exists, so the lease clock falls back to `started_at`.
    running_job(&storage, PHANTOM_JOB, SILENT_SECONDS, 0, None);

    let out = tick(&storage);
    let log = transcript(&out);

    assert!(
        out.status.success(),
        "a tick that heals the queue exits 0: {log}"
    );
    assert!(
        log.contains(&format!(
            "{PHANTOM_JOB}: requeued (worker lease expired; lease silent for"
        )) && log.contains("; restart 1/20)"),
        "the tick names the job, the reason and the retry budget: {log}"
    );
    assert!(
        log.contains(
            "lease-reaper: requeued 1 phantom job(s), failed 0 on second expiry, \
             cleared 0 silent-worker assignment(s)"
        ),
        "the summary counts one requeue and no failure: {log}"
    );

    // State, not stdout, is the contract: the record moved back to the
    // queue with the expiry stored on it, and nothing lingers in running/.
    let requeued = read(&storage, &format!("queue/{PHANTOM_JOB}.json"));
    assert_eq!(requeued["state"], "queued");
    assert_eq!(requeued["restarts"], 1);
    assert_eq!(requeued["error"], LEASE_EXPIRED);
    assert!(
        !storage.join(format!("running/{PHANTOM_JOB}.json")).exists(),
        "the phantom is gone from running/"
    );
}

#[test]
fn second_lease_expiry_fails_the_job_instead_of_looping_forever() {
    let (_home, storage) = fleet();
    // The record a first expiry leaves behind, claimed again and orphaned
    // again: the stored reason is the marker the reaper matches on.
    running_job(&storage, SECOND_PHANTOM, SILENT_SECONDS, 1, Some(LEASE_EXPIRED));
    run_receipt(&storage, SECOND_PHANTOM);

    let out = tick(&storage);
    let log = transcript(&out);

    assert!(out.status.success(), "failing a job is a verdict, not a crash: {log}");
    assert!(
        log.contains(&format!(
            "{SECOND_PHANTOM}: FAILED (worker lease expired; lease silent for"
        )) && log.contains("; second lease expiry)"),
        "the tick states why the job is failed now: {log}"
    );
    assert!(
        log.contains("failed 1 on second expiry"),
        "the summary counts the terminal verdict: {log}"
    );

    // The job is terminal: it must never return to the live prefixes. The
    // run-reaper folds the outcome into the batch receipt in the same tick
    // and cleans up the per-job blob, so the receipt is the durable record.
    assert!(!storage.join(format!("running/{SECOND_PHANTOM}.json")).exists());
    assert!(!storage.join(format!("queue/{SECOND_PHANTOM}.json")).exists());
    let receipt = read(&storage, &format!("runs/run-{SECOND_PHANTOM}.json"));
    assert_eq!(
        receipt["final_counts"]["failed"], 1,
        "the batch receipt records the failure: {receipt}"
    );
    assert!(
        receipt["reaped_at"].is_string(),
        "the receipt is closed out: {receipt}"
    );
}

#[test]
fn a_fresh_lease_is_never_touched() {
    let (_home, storage) = fleet();
    // A worker claimed this job moments ago; its lease is well inside the
    // 15-minute TTL even without a heartbeat blob yet.
    running_job(&storage, PHANTOM_JOB, 30, 0, None);

    let out = tick(&storage);
    let log = transcript(&out);

    assert!(out.status.success(), "{log}");
    assert!(
        !log.contains("lease-reaper:"),
        "an idle reaper says nothing: {log}"
    );
    let untouched = read(&storage, &format!("running/{PHANTOM_JOB}.json"));
    assert_eq!(untouched["state"], "running");
    assert_eq!(untouched["restarts"], 0);
    assert_eq!(untouched["error"], Value::Null);
    assert!(
        !storage.join(format!("queue/{PHANTOM_JOB}.json")).exists(),
        "a live job is never requeued"
    );
}

#[test]
fn an_unreachable_registry_is_a_stated_skip_not_a_crash() {
    let (_home, storage) = fleet();
    // No registry.json exists anywhere: the authority is unreachable and
    // there is no last-known-good copy. The build poll must degrade to a
    // named skip while the rest of the tick (the reaper included) runs.
    running_job(&storage, PHANTOM_JOB, SILENT_SECONDS, 0, None);

    let out = tick(&storage);
    let log = transcript(&out);

    assert!(
        out.status.success(),
        "a dead authority degrades the tick, it does not kill it: {log}"
    );
    assert!(
        log.contains(
            "build poll skipped: registry unreachable: \
             no registry document at local:registry.json"
        ),
        "the skip is stated with its cause: {log}"
    );
    assert!(
        log.contains(&format!(
            "{PHANTOM_JOB}: requeued (worker lease expired; lease silent for"
        )),
        "the reaper still healed the queue in the same tick: {log}"
    );
}
