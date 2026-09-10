//! The real local-worker capacity journeys.
use super::*;

#[test]
#[ignore = "Probierz records the real local-worker capacity journey"]
fn live_resources_admit_two_jobs_despite_legacy_single_worker_limits() {
    let mut journey = Journey::new();
    let first = journey.submit_blocked("first");
    let second = journey.submit_blocked("second");
    journey.start_agent();

    journey.wait_for(
        "both workloads to be running",
        Duration::from_secs(45),
        |state| {
            state.home.path().join("first.started").exists()
                && state.home.path().join("second.started").exists()
                && !state.home.path().join("release").exists()
        },
    );
    journey.wait_for(
        "a capacity publication describing both running jobs",
        Duration::from_secs(30),
        |state| {
            state.newest_capacity().is_some_and(|capacity| {
                capacity["running_jobs"]
                    .as_i64()
                    .is_some_and(|count| count >= 2)
                    && capacity["total_cpu_cores"]
                        .as_i64()
                        .is_some_and(|count| count >= 2)
                    && capacity.get("available_cpu_cores").is_some()
                    && capacity.get("free_ram_gb").is_some()
                    && capacity.get("available_accelerators").is_some()
                    && capacity.get("accepting_jobs").is_some()
                    && capacity.get("free_slots").is_none()
            })
        },
    );

    fs::write(journey.home.path().join("release"), b"go\n").unwrap();
    journey.wait_for("both jobs to finish", Duration::from_secs(45), |state| {
        state
            .storage
            .join("completed")
            .join(format!("{first}.json"))
            .exists()
            && state
                .storage
                .join("completed")
                .join(format!("{second}.json"))
                .exists()
            && state.home.path().join("first.finished").exists()
            && state.home.path().join("second.finished").exists()
    });
}

/// A host whose own declaration refuses placement publishes the numbers behind
/// that refusal, and the claiming verdict names it.
///
/// On 2026-09-10 the only Linux builder refused every job for memory pressure,
/// `skarbiec` could not publish `linux-amd64`, and `stado host gates` answered
/// 62.8 of 123.0 GiB free RAM with no watermark, no swap figure and no memory
/// blocker at all: the reason sat in `diag.admission_reason` and no surface
/// read it.
#[test]
fn a_declared_memory_refusal_publishes_its_numbers_and_blocks_claiming() {
    let mut journey = Journey::new();
    journey.invoke_ok(&[
        "space",
        "watermark",
        TARGET,
        "--memory-mode",
        "report",
        "--memory-low-free-mb",
        "1048576",
        "--memory-target-free-mb",
        "2097152",
        "--memory-high-swap-used-pct",
        "100",
        "--memory-refuse-placement",
        "true",
    ]);
    journey.invoke_ok(&["disk-cleanup", "--once"]);
    journey.start_agent();
    journey.wait_for("a capacity publication", Duration::from_secs(30), |state| {
        state.newest_capacity().is_some()
    });
    let capacity = journey.newest_capacity().expect("a published capacity");
    assert_eq!(capacity["accepting_jobs"], json!(false), "{capacity}");
    let diag = &capacity["diag"];
    assert_eq!(diag["admission_reason"], json!("memory_pressure_active"));
    assert_eq!(diag["memory_refuse_placement"], json!(true));
    assert!(
        diag["memory_available_gb"].as_f64().is_some(),
        "the refusal published no reading: {diag}"
    );
    assert!(
        diag["memory_low_watermark_gb"].as_f64().is_some(),
        "the refusal published no watermark: {diag}"
    );
    let verdict = journey.invoke(&["host", "gates", TARGET]);
    let text = String::from_utf8_lossy(&verdict.stdout).into_owned();
    assert!(
        text.contains("memory_pressure_active"),
        "the verdict hid the refusal: {text}"
    );
    assert!(
        text.lines()
            .any(|line| line.starts_with("memory:") && line.contains("refusing placement")),
        "the report printed no memory line: {text}"
    );
}

/// A host whose cleanup lock is held publishes that it is not accepting
/// jobs, and names the janitor as the reason.
///
/// On 2026-09-10 lukasz-macbook's janitor pass held the exclusive cleanup
/// lock for two hours inside a directory open macOS had parked behind a
/// consent dialog. Every claim answered `cleanup_in_progress` and took
/// nothing, the capacity publication kept saying `accepting_jobs: true`,
/// `stado host gates` read `claiming: yes`, and the queued release delivery
/// sat pinned to a host that could not start it. The publication is the
/// document the coordinator and the gate judge by, so it has to say what
/// the claim will do.
#[test]
fn a_held_cleanup_lock_publishes_cleanup_in_progress_and_blocks_claiming() {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let mut journey = Journey::new();
    let state_dir = journey.home.path().join(".cache").join("wisent-compute");
    fs::create_dir_all(&state_dir).unwrap();
    fs::set_permissions(&state_dir, fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(
        journey.home.path().join(".cache"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(state_dir.join("disk-cleanup.lock"))
        .unwrap();
    // What a janitor pass holds for its whole duration.
    fs2::FileExt::lock_exclusive(&lock).unwrap();

    journey.start_agent();
    journey.wait_for(
        "a capacity publication naming the held cleanup lock",
        Duration::from_secs(30),
        |state| {
            state.newest_capacity().is_some_and(|capacity| {
                capacity["diag"]["admission_reason"] == json!("cleanup_in_progress")
            })
        },
    );
    let capacity = journey.newest_capacity().expect("a published capacity");
    assert_eq!(capacity["accepting_jobs"], json!(false), "{capacity}");
    let verdict = journey.invoke(&["host", "gates", TARGET]);
    let text = String::from_utf8_lossy(&verdict.stdout).into_owned();
    assert!(
        text.contains("cleanup_in_progress"),
        "the verdict hid the refusal: {text}"
    );
    assert!(
        text.lines()
            .any(|line| line.starts_with("claiming:") && line.contains("no")),
        "the verdict still said the host claims: {text}"
    );

    // Releasing the lock is the whole remedy: the next publication admits.
    fs2::FileExt::unlock(&lock).unwrap();
    journey.wait_for(
        "a publication that admits again",
        Duration::from_secs(30),
        |state| {
            state
                .newest_capacity()
                .is_some_and(|capacity| capacity["accepting_jobs"] == json!(true))
        },
    );
}
