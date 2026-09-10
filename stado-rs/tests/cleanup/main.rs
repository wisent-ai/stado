//! Real `stado disk-cleanup` journeys against an isolated home and store.
//!
//! The tests drive the built product binary. A Cache Directory Tagging
//! Standard directory supplies genuinely regenerable state; the assertions read
//! the filesystem and persisted janitor report after preview and enforcement.

mod support;

use std::fs::{self, OpenOptions};

use fs2::FileExt;
use serde_json::{json, Value};

use support::Journey;

#[test]
#[ignore = "Probierz records the real disk-cleanup preview journey"]
fn dry_run_reports_eligible_cache_without_deleting_or_persisting() {
    let journey = Journey::new();
    let tagged = journey.tagged_cache("preview-candidate");
    let untagged = journey.untagged_directory();

    let report = journey.invoke_ok(&["disk-cleanup", "--dry-run"]);

    assert_eq!(report["mode"], "report", "cleanup report: {report:#}");
    assert_eq!(report["outcome"], "report_only");
    assert_eq!(report["cleaners"]["build_caches"]["eligible_items"], 1);
    assert_eq!(report["cleaners"]["build_caches"]["deleted_items"], 0);
    assert_eq!(report["active_job_count"], 0);
    assert!(tagged.is_dir(), "preview must preserve an eligible cache");
    assert!(untagged.is_dir(), "preview must preserve unrelated source");
    assert!(
        !journey.state_path().exists(),
        "preview must write no janitor state"
    );
}

#[test]
#[ignore = "Probierz records the real disk-cleanup enforcement journey"]
fn enforce_deletes_only_tagged_cache_and_persists_reclaimed_progress() {
    let journey = Journey::new();
    let tagged = journey.tagged_cache("enforce-candidate");
    let untagged = journey.untagged_directory();

    let report = journey.invoke_ok(&["disk-cleanup", "--once"]);

    assert_eq!(report["mode"], "enforce", "cleanup report: {report:#}");
    assert_eq!(report["outcome"], "reclaimed_progress");
    assert_eq!(report["cleaners"]["build_caches"]["eligible_items"], 1);
    assert_eq!(report["cleaners"]["build_caches"]["deleted_items"], 1);
    assert_eq!(report["errors"], json!([]));
    assert!(
        !tagged.exists(),
        "enforcement must remove the eligible cache"
    );
    assert!(
        untagged.join("valuable-source.txt").is_file(),
        "enforcement must preserve an untagged source directory"
    );

    let state: Value = serde_json::from_slice(&fs::read(journey.state_path()).unwrap()).unwrap();
    assert_eq!(state["report"]["outcome"], "reclaimed_progress");
    assert_eq!(state["report"]["writer"], "disk-cleanup-cli");
}

#[test]
#[ignore = "Probierz records the real stale-lock recovery journey"]
fn overdue_lock_stays_report_only_until_the_predecessor_kernel_lock_is_released() {
    let journey = Journey::new();
    let tagged = journey.tagged_cache("lock-recovery-candidate");
    let state_dir = journey.state_dir();
    fs::create_dir_all(&state_dir).unwrap();
    let lock_path = state_dir.join("disk-cleanup.lock");
    let held = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .unwrap();
    held.lock_exclusive().unwrap();
    fs::write(
        state_dir.join("disk-cleanup.lock.holder"),
        serde_json::to_vec(&json!({
            "pid": std::process::id(),
            "acquired_at": 0.0,
            "deadline_at": 0.0,
            "writer": "legacy-agent",
            "writer_version": "legacy"
        }))
        .unwrap(),
    )
    .unwrap();

    let takeover = journey.invoke_ok(&["disk-cleanup", "--once"]);
    assert_eq!(takeover["outcome"], "lock_recovery_report_only");
    assert!(tagged.is_dir(), "takeover pass must not delete");
    assert_eq!(journey.retired_locks().len(), 1);
    let persisted: Value =
        serde_json::from_slice(&fs::read(journey.state_path()).unwrap()).unwrap();
    assert_eq!(persisted["report"]["outcome"], "lock_recovery_report_only");

    let still_held = journey.invoke_ok(&["disk-cleanup", "--once"]);
    assert_eq!(still_held["outcome"], "lock_recovery_report_only");
    assert!(
        tagged.is_dir(),
        "a second pass must not delete through the replacement lock"
    );

    FileExt::unlock(&held).unwrap();
    drop(held);
    let recovered = journey.invoke_ok(&["disk-cleanup", "--once"]);
    assert_eq!(
        recovered["mode"], "enforce",
        "cleanup report: {recovered:#}"
    );
    assert_eq!(recovered["outcome"], "reclaimed_progress");
    assert!(
        !tagged.exists(),
        "enforcement resumes only after the predecessor releases its inode"
    );
    assert!(journey.retired_locks().is_empty());
}

#[test]
#[ignore = "Probierz records the real busy-lock state-preservation journey"]
fn busy_lock_preserves_the_reclaim_hysteresis_and_scan_cursor() {
    let journey = Journey::new();
    let state_dir = journey.state_dir();
    fs::create_dir_all(&state_dir).unwrap();
    fs::write(
        journey.state_path(),
        serde_json::to_vec(&json!({
            "version": 1,
            "last_attempt_at": 0.0,
            "report": {
                "target_name": "cleanup-runner",
                "policy_digest": "continuing-policy",
                "policy_defaulted": false,
                "mode": "enforce",
                "check_interval_seconds": 60,
                "low_bytes": 1073741824_i64,
                "target_bytes": 2147483648_i64,
                "pressure_active": true,
                "last_success_at": "2026-09-04T00:00:00+00:00",
                "build_caches_resume_from": "project/target",
                "unscanned_cleaners": ["build_caches"]
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let held = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(state_dir.join("disk-cleanup.lock"))
        .unwrap();
    held.lock_exclusive().unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    fs::write(
        state_dir.join("disk-cleanup.lock.holder"),
        serde_json::to_vec(&json!({
            "pid": std::process::id(),
            "acquired_at": now,
            "deadline_at": now + 3600.0,
            "writer": "active-agent",
            "writer_version": "current"
        }))
        .unwrap(),
    )
    .unwrap();

    let report = journey.invoke_ok(&["disk-cleanup", "--once"]);
    assert_eq!(report["outcome"], "lock_busy");
    assert_eq!(report["policy_digest"], "continuing-policy");
    assert_eq!(report["pressure_active"], true);
    assert_eq!(report["build_caches_resume_from"], "project/target");
    assert_eq!(report["unscanned_cleaners"], json!(["build_caches"]));
    let persisted: Value =
        serde_json::from_slice(&fs::read(journey.state_path()).unwrap()).unwrap();
    assert_eq!(persisted["report"]["policy_digest"], "continuing-policy");
    assert_eq!(persisted["report"]["pressure_active"], true);
    assert_eq!(
        persisted["report"]["build_caches_resume_from"],
        "project/target"
    );

    FileExt::unlock(&held).unwrap();
}

#[test]
#[ignore = "Probierz records the disk-cleanup CLI refusal"]
fn once_and_watch_are_refused_with_the_public_usage_sentence() {
    let journey = Journey::new();
    let output = journey.invoke(&["disk-cleanup", "--once", "--watch"]);

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(
        stderr.lines().next(),
        Some("Error: --once and --watch are mutually exclusive")
    );
}
