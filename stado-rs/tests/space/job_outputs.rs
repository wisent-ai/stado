//! The job-outputs cleaner against a real local queue store: only the aged
//! payload of a job the queue has retired goes; records, fresh payloads, live
//! jobs and jobs the queue does not list stay.

use std::fs::{self, File, FileTimes};
use std::path::Path;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use crate::fixture::{Host, JANITOR_STATE};

/// The retention floor the catalogue declares for this cleaner, in seconds
/// (seven days), and the age this case gives an "old" file (nine days) so
/// the floor is the only line between the two payloads of the retired job.
const FLOOR_SECONDS: u64 = 604_800;
const OLD_SECONDS: u64 = 9 * 24 * 60 * 60;

fn backdate(path: &Path) {
    let aged = SystemTime::now() - Duration::from_secs(OLD_SECONDS);
    File::open(path)
        .expect("open the file to age it")
        .set_times(FileTimes::new().set_accessed(aged).set_modified(aged))
        .expect("age the file past the retention floor");
}

fn seed_output(storage: &Path, job_id: &str, aged: bool) {
    let output = storage.join("status").join(job_id).join("output");
    fs::create_dir_all(&output).expect("create the job's output directory");
    for name in ["release.tar.gz", "receipt.json", "command_output.log"] {
        fs::write(output.join(name), format!("{job_id} {name}")).expect("write an output");
        if aged {
            backdate(&output.join(name));
        }
    }
}

#[test]
fn a_retired_jobs_aged_payload_is_reclaimed_and_everything_else_stays() {
    let host = Host::new();
    let mut policy: Value = serde_json::from_str(&host.policy()).unwrap();
    policy["cleaners"] = json!({ "job_outputs": { "min_age_seconds": FLOOR_SECONDS } });
    host.declare(&policy.to_string());

    let storage = &host.storage;
    fs::create_dir_all(storage.join("completed")).unwrap();
    fs::create_dir_all(storage.join("queue")).unwrap();
    fs::write(storage.join("completed/job-done.json"), b"{}").unwrap();
    fs::write(storage.join("queue/job-live.json"), b"{}").unwrap();
    seed_output(storage, "job-done", true);
    seed_output(storage, "job-live", true);
    seed_output(storage, "job-unlisted", true);
    let fresh = storage.join("status/job-done/output/fresh.bin");
    fs::write(&fresh, b"still being read").unwrap();

    let output = host.run(&["disk-cleanup", "--to-target"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let done = storage.join("status/job-done/output");
    assert!(
        !done.join("release.tar.gz").exists(),
        "the retired job's aged payload stays"
    );
    assert!(
        fresh.exists(),
        "a payload younger than the floor was removed"
    );
    assert!(
        done.join("receipt.json").exists(),
        "the receipt was removed"
    );
    assert!(
        done.join("command_output.log").exists(),
        "the log was removed"
    );
    for job in ["job-live", "job-unlisted"] {
        let payload = storage
            .join("status")
            .join(job)
            .join("output/release.tar.gz");
        assert!(payload.exists(), "{job}'s payload was removed");
    }

    let state: Value = serde_json::from_str(
        &fs::read_to_string(host.under_home(JANITOR_STATE)).expect("the janitor wrote its state"),
    )
    .unwrap();
    let cleaner = &state["report"]["cleaners"]["job_outputs"];
    assert_eq!(cleaner["deleted_items"], 1, "{state}");
    assert_eq!(cleaner["skipped"]["record_kept"], 2, "{state}");
    assert_eq!(cleaner["skipped"]["younger_than_min_age"], 1, "{state}");
}
