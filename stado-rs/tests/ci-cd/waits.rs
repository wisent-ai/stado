//! What the journeys observe in the store while the product works.
//!
//! Each wait reads state the product wrote — a capacity broadcast, a queued
//! job, a cancellation record — and each refusal prints the store it was
//! looking at, because a release that did not happen is only diagnosable from
//! the queue it never reached.

use std::fs::{self, File};
use std::path::Path;
use std::process::{Child, ExitStatus};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};

use crate::fixture::ReleaseFixture;

/// How long a builder may take to publish capacity it will claim. The first
/// broadcast an agent publishes carries no processor measurement at all —
/// `available_cpu_cores` needs two samples of the kernel's cumulative counters
/// — so the wait below has to outlast at least one further publish cycle.
const CLAIMABLE_CAPACITY_TIMEOUT: Duration = Duration::from_secs(300);
/// How long `stado release submit` may take to queue work it has accepted.
const QUEUEING_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a real `cargo check` plus `cargo build --release` plus signing,
/// publication and delivery may take on this host.
const RELEASE_TIMEOUT: Duration = Duration::from_secs(180);
const POLL: Duration = Duration::from_millis(100);

pub fn store_snapshot(storage: &Path) -> String {
    let mut out = String::new();
    for prefix in ["queue", "running", "failed", "completed", "capacity"] {
        let path = storage.join(prefix);
        let Ok(entries) = fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            out.push_str(&format!(
                "\n== {prefix}/{} ==\n{}",
                entry.file_name().to_string_lossy(),
                fs::read_to_string(entry.path()).unwrap_or_else(|_| "<binary>".into())
            ));
        }
    }
    out
}

fn published_capacity(storage: &Path) -> Vec<Value> {
    let Ok(entries) = fs::read_dir(storage.join("capacity")) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| fs::read(entry.path()).ok())
        .filter_map(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .collect()
}

/// Wait until a builder publishes capacity it will actually claim work from.
///
/// Waiting for the mere existence of a capacity file is what kept these
/// journeys from ever running: the first broadcast always carries
/// `admission_reason: cpu_measurement_unavailable`, and `release submit`
/// answers such a fleet with "no live fleet builder can CLAIM
/// release_platform".
pub fn wait_for_claimable_capacity(fixture: &ReleaseFixture, agent: &mut Child) {
    let deadline = Instant::now() + CLAIMABLE_CAPACITY_TIMEOUT;
    loop {
        if published_capacity(&fixture.storage)
            .iter()
            .any(|capacity| capacity["accepting_jobs"] == true)
        {
            return;
        }
        if let Some(status) = agent.try_wait().unwrap() {
            panic!(
                "agent exited before accepting work: {status}\nstdout:\n{}\nstderr:\n{}",
                fixture.read("agent.out"),
                fixture.read("agent.err")
            );
        }
        if Instant::now() >= deadline {
            let refusals: Vec<String> = published_capacity(&fixture.storage)
                .iter()
                .map(|capacity| {
                    format!(
                        "{}: admission_reason={} cpu_load_1m={}",
                        capacity["consumer_id"],
                        capacity["diag"]["admission_reason"],
                        capacity["diag"]["cpu_load_1m"]
                    )
                })
                .collect();
            let _ = agent.kill();
            let _ = agent.wait();
            panic!(
                "this host published capacity for {} seconds without ever accepting work: {}. \
                 A builder with no spare processor capacity cannot compile a release, so this \
                 journey needs a machine that is not already saturated.\nstore:{}",
                CLAIMABLE_CAPACITY_TIMEOUT.as_secs(),
                refusals.join("; "),
                store_snapshot(&fixture.storage)
            );
        }
        thread::sleep(POLL);
    }
}

/// A capacity broadcast for a target that stopped reporting: published four
/// minutes ago, under disk pressure, offering no slots. Written by hand
/// because the target it names is deliberately unreachable.
pub fn seed_stale_capacity(storage: &Path, consumer: &str) {
    let capacity = storage.join("capacity");
    fs::create_dir_all(&capacity).unwrap();
    let path = capacity.join(format!("{consumer}.json"));
    fs::write(
        &path,
        serde_json::to_vec_pretty(&json!({
            "consumer_id": consumer,
            "kind": "local",
            "published_at": "2026-01-01T00:00:00Z",
            "free_slots": {},
            "diag": {
                "disk_pressure_active": true,
                "disk_pressure_unresolved": true
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let stale = SystemTime::now() - Duration::from_secs(240);
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(stale))
        .unwrap();
}

pub fn wait_for_recovery_delivery(
    fixture: &ReleaseFixture,
    submit: &mut Child,
    agent: &mut Child,
    consumer: &str,
) -> Value {
    let deadline = Instant::now() + RELEASE_TIMEOUT;
    loop {
        if let Some(job) = queued_job(fixture, |job| {
            job["command"] == stado::constants::PRODUCT_RELEASE_DELIVERY_JOB_COMMAND
                && job["pinned_host"] == consumer
        }) {
            return job;
        }
        if let Some(status) = submit.try_wait().unwrap() {
            panic!(
                "release submit exited before queuing the recovery delivery: {status}\n\
                 submit stdout:\n{}\nsubmit stderr:\n{}\nstore:{}",
                fixture.read("submit.out"),
                fixture.read("submit.err"),
                store_snapshot(&fixture.storage)
            );
        }
        if let Some(status) = agent.try_wait().unwrap() {
            let _ = submit.kill();
            panic!(
                "builder exited before the recovery delivery was queued: {status}\n\
                 agent stdout:\n{}\nagent stderr:\n{}",
                fixture.read("agent.out"),
                fixture.read("agent.err")
            );
        }
        if Instant::now() >= deadline {
            let _ = submit.kill();
            let _ = submit.wait();
            panic!(
                "release submit queued no recovery delivery within {} seconds\n\
                 submit stdout:\n{}\nsubmit stderr:\n{}\nagent stderr:\n{}\nstore:{}",
                RELEASE_TIMEOUT.as_secs(),
                fixture.read("submit.out"),
                fixture.read("submit.err"),
                fixture.read("agent.err"),
                store_snapshot(&fixture.storage)
            );
        }
        thread::sleep(POLL);
    }
}

fn queued_job(fixture: &ReleaseFixture, matches: impl Fn(&Value) -> bool) -> Option<Value> {
    let entries = fs::read_dir(fixture.storage.join("queue")).ok()?;
    entries
        .flatten()
        .filter_map(|entry| fs::read(entry.path()).ok())
        .filter_map(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .find(|job| matches(job))
}

/// The queued release build for this source.
///
/// The worker invocation is matched inside the queued command rather than at
/// its end: the product wraps `stado release worker` in a bootstrap script
/// that prepares the persistent workdir, uploads the evidence objects and
/// follows the job's lifecycle, so the command ends in that script's `exit`.
pub fn wait_for_queued_release_build(fixture: &ReleaseFixture, submit: &mut Child) -> Value {
    const WORKER_INVOCATION: &str = "release worker --request release-request.json";
    let deadline = Instant::now() + QUEUEING_TIMEOUT;
    loop {
        if let Some(job) = queued_job(fixture, |job| {
            job["command"]
                .as_str()
                .is_some_and(|command| command.contains(WORKER_INVOCATION))
        }) {
            return job;
        }
        if let Some(status) = submit.try_wait().unwrap() {
            panic!(
                "release submit exited before queuing its build: {status}\n\
                 submit stdout:\n{}\nsubmit stderr:\n{}\nstore:{}",
                fixture.read("submit-first.out"),
                fixture.read("submit-first.err"),
                store_snapshot(&fixture.storage)
            );
        }
        assert!(
            Instant::now() < deadline,
            "release submit queued no build within {} seconds\nstore:{}",
            QUEUEING_TIMEOUT.as_secs(),
            store_snapshot(&fixture.storage)
        );
        thread::sleep(POLL);
    }
}

pub fn wait_for_submit(
    fixture: &ReleaseFixture,
    submit: &mut Child,
    agent: &mut Child,
) -> ExitStatus {
    let deadline = Instant::now() + RELEASE_TIMEOUT;
    loop {
        if let Some(status) = submit.try_wait().unwrap() {
            return status;
        }
        if let Some(status) = agent.try_wait().unwrap() {
            let _ = submit.kill();
            panic!(
                "agent exited while release submit waited: {status}\n\
                 agent stdout:\n{}\nagent stderr:\n{}",
                fixture.read("agent.out"),
                fixture.read("agent.err")
            );
        }
        if Instant::now() >= deadline {
            let _ = submit.kill();
            let _ = submit.wait();
            panic!(
                "release submit did not finish within {} seconds\n\
                 submit stdout:\n{}\nsubmit stderr:\n{}\nagent stderr:\n{}\nstore:{}",
                RELEASE_TIMEOUT.as_secs(),
                fixture.read("submit.out"),
                fixture.read("submit.err"),
                fixture.read("agent.err"),
                store_snapshot(&fixture.storage)
            );
        }
        thread::sleep(POLL);
    }
}

/// Wait for a submit that has been cancelled to give up, which it must do
/// without being killed.
pub fn wait_for_cancelled_submit(fixture: &ReleaseFixture, submit: &mut Child) -> ExitStatus {
    let deadline = Instant::now() + QUEUEING_TIMEOUT;
    loop {
        if let Some(status) = submit.try_wait().unwrap() {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "cancelled release submit did not exit\nstore:{}",
            store_snapshot(&fixture.storage)
        );
        thread::sleep(POLL);
    }
}
