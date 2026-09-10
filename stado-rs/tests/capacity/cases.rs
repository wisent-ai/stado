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
