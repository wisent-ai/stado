//! Byte-parity tests against Python-generated `Job.to_json()` fixtures.
//! Fixtures are produced by the Python package (see repo history / phase-0
//! plan) and must round-trip through the Rust model byte-identically.

use stado::models::Job;

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR")))
        .expect("fixture readable")
}

#[test]
fn python_minimal_job_round_trips_byte_identically() {
    let original = fixture("job_minimal.json");
    let job = Job::from_json(&original).unwrap();
    assert_eq!(job.to_json(), original);
}

#[test]
fn python_full_job_round_trips_byte_identically() {
    let original = fixture("job_full.json");
    let job = Job::from_json(&original).unwrap();
    assert_eq!(job.to_json(), original);
}

#[test]
fn legacy_partial_record_gets_python_defaults() {
    let job = Job::from_json(&fixture("job_legacy_partial.json")).unwrap();
    assert_eq!(job.job_id, "legacy");
    assert_eq!(job.state, "completed");
    // Python dataclass defaults for missing keys:
    assert_eq!(job.provider, "gcp");
    assert_eq!(job.max_restarts, 20);
    assert_eq!(job.boot_disk_gb, 500);
    assert_eq!(job.repo_extras, "train");
    assert_eq!(job.executor, "stado-agent");
    assert_eq!(job.yield_grace_seconds, 120);
    assert!(!job.created_at.is_empty(), "existing created_at preserved");
    assert_eq!(job.created_at, "2025-05-01T00:00:00+00:00");
}
