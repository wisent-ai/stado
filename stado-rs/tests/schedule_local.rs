//! End-to-end `stado schedule ...` tests against the local storage backend.
//!
//! CLI-level tests drive the built `stado` binary (`CARGO_BIN_EXE_stado`)
//! with WC_STORAGE_BACKEND=local + WC_LOCAL_STORAGE_PATH=<TempDir>. The
//! firing tests call `schedules::fire_due_schedules` in-process (the CLI
//! has no tick command — firing is the coordinator's job), with the same
//! env pinning so `submit_job` lands in the same TempDir.

use std::path::Path;
use std::process::{Command, Output};

use chrono::{Duration, Utc};
use serde_json::Value;
use stado::models::Job;
use stado::queue::JobStorage;
use stado::schedules::{self, Schedule};

fn stado(storage: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_stado"));
    cmd.args(args)
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", storage)
        // A set-but-missing STADO_CONFIG disables config-file discovery.
        .env("STADO_CONFIG", storage.join("no-such-config.json"))
        .env_remove("COMPUTE_API_KEY")
        .env_remove("COMPUTE_API_URL")
        .env_remove("WC_PROFILES_DIR");
    cmd.output().expect("stado binary runs")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Extract the `sch-<hex>` id from `created schedule <sid> (...)`.
fn created_sid(out: &Output) -> String {
    stdout(out)
        .lines()
        .find_map(|line| line.strip_prefix("created schedule "))
        .and_then(|rest| rest.split_whitespace().next())
        .expect("create echoed a schedule id")
        .to_string()
}

fn read_schedule_json(storage: &Path, sid: &str) -> Value {
    let raw = std::fs::read_to_string(storage.join("schedules").join(format!("{sid}.json")))
        .expect("schedule blob exists");
    serde_json::from_str(&raw).expect("schedule blob is JSON")
}

#[test]
fn schedule_create_list_show_pause_resume_rm() {
    let dir = tempfile::tempdir().unwrap();
    let storage = dir.path().join("storage");
    std::fs::create_dir_all(&storage).unwrap();

    // Invalid cron is refused with a click-style error.
    let out = stado(
        &storage,
        &["schedule", "create", "echo x", "--cron", "not a cron"],
    );
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("Error: invalid cron expression: 'not a cron'"),
        "{}",
        stderr(&out)
    );

    // Create with a tz and routing flags.
    let out = stado(
        &storage,
        &[
            "schedule",
            "create",
            "echo scheduled-job",
            "--cron",
            "30 3 * * 1-5",
            "--tz",
            "Europe/Warsaw",
            "--priority",
            "4",
            "--gpu-type",
            "nvidia-l4",
            "--apt",
            "htop, git-lfs",
            "--overlap-policy",
            "allow",
        ],
    );
    assert!(out.status.success(), "create failed: {}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains(" (enabled)"), "{text}");
    assert!(
        text.contains("  cron:     30 3 * * 1-5  (Europe/Warsaw)"),
        "{text}"
    );
    assert!(text.contains("  next run: "), "{text}");
    assert!(text.contains("  command:  echo scheduled-job"), "{text}");
    let sid = created_sid(&out);
    assert!(sid.starts_with("sch-"), "{sid}");

    // The blob holds the frozen submit payload.
    let json = read_schedule_json(&storage, &sid);
    assert_eq!(json["cron"], Value::from("30 3 * * 1-5"));
    assert_eq!(json["tz"], Value::from("Europe/Warsaw"));
    assert_eq!(json["command"], Value::from("echo scheduled-job"));
    assert_eq!(json["enabled"], Value::from(true));
    assert_eq!(json["priority"], Value::from(4));
    assert_eq!(json["gpu_type"], Value::from("nvidia-l4"));
    assert_eq!(json["apt_packages"], serde_json::json!(["htop", "git-lfs"]));
    assert_eq!(json["overlap_policy"], Value::from("allow"));
    assert!(
        json["next_due_at"].as_str().unwrap().ends_with("+00:00"),
        "{json}"
    );

    // List shows the table row.
    let out = stado(&storage, &["schedule", "list"]);
    assert!(out.status.success());
    let text = stdout(&out);
    assert!(text.contains("ID"), "{text}");
    assert!(text.contains(&sid), "{text}");
    assert!(text.contains("30 3 * * 1-5"), "{text}");
    assert!(text.contains("1 schedule(s)"), "{text}");

    // Show prints the full JSON.
    let out = stado(&storage, &["schedule", "show", &sid]);
    assert!(out.status.success());
    let shown: Value = serde_json::from_str(stdout(&out).trim()).unwrap();
    assert_eq!(shown["schedule_id"], Value::from(sid.as_str()));

    // Pause disables; resume re-enables with a recomputed next run.
    let out = stado(&storage, &["schedule", "pause", &sid]);
    assert!(out.status.success());
    assert_eq!(stdout(&out).trim(), format!("paused {sid}"));
    assert_eq!(
        read_schedule_json(&storage, &sid)["enabled"],
        Value::from(false)
    );

    let out = stado(&storage, &["schedule", "resume", &sid]);
    assert!(out.status.success());
    assert!(
        stdout(&out).starts_with(&format!("resumed {sid}; next run ")),
        "{}",
        stdout(&out)
    );
    assert_eq!(
        read_schedule_json(&storage, &sid)["enabled"],
        Value::from(true)
    );

    // rm deletes; a second rm is a click-style not-found error.
    let out = stado(&storage, &["schedule", "rm", &sid]);
    assert!(out.status.success());
    assert_eq!(stdout(&out).trim(), format!("deleted schedule {sid}"));
    assert!(!storage
        .join("schedules")
        .join(format!("{sid}.json"))
        .exists());
    let out = stado(&storage, &["schedule", "rm", &sid]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains(&format!("Error: schedule {sid} not found")),
        "{}",
        stderr(&out)
    );

    // Empty listing.
    let out = stado(&storage, &["schedule", "list"]);
    assert_eq!(stdout(&out).trim(), "(no schedules)");
}

#[test]
fn schedule_create_disabled_starts_paused() {
    let dir = tempfile::tempdir().unwrap();
    let storage = dir.path().join("storage");
    std::fs::create_dir_all(&storage).unwrap();
    let out = stado(
        &storage,
        &[
            "schedule",
            "create",
            "echo later",
            "--cron",
            "0 0 * * *",
            "--disabled",
        ],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains(" (DISABLED)"), "{}", stdout(&out));
    let sid = created_sid(&out);
    assert_eq!(
        read_schedule_json(&storage, &sid)["enabled"],
        Value::from(false)
    );
}

#[test]
fn schedule_run_fires_job_tagged_with_schedule_id() {
    let dir = tempfile::tempdir().unwrap();
    let storage = dir.path().join("storage");
    std::fs::create_dir_all(&storage).unwrap();
    let out = stado(
        &storage,
        &["schedule", "create", "echo fire-me", "--cron", "0 2 * * *"],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let sid = created_sid(&out);

    let out = stado(&storage, &["schedule", "run", &sid]);
    assert!(out.status.success(), "run failed: {}", stderr(&out));
    let text = stdout(&out);
    let job_id = text
        .trim()
        .strip_prefix(&format!("fired {sid} -> job "))
        .and_then(|rest| rest.split_whitespace().next())
        .expect("run echoed the fired job id")
        .to_string();
    assert!(text.contains("(run run-"), "{text}");

    // The queued job carries schedule_id and a fresh run_id.
    let raw =
        std::fs::read_to_string(storage.join("queue").join(format!("{job_id}.json"))).unwrap();
    let job = Job::from_json(&raw).unwrap();
    assert_eq!(job.schedule_id, sid);
    assert!(job.run_id.starts_with("run-"), "{:?}", job.run_id);
    assert_eq!(job.command, "echo fire-me");

    // The schedule recorded the fire.
    let json = read_schedule_json(&storage, &sid);
    assert_eq!(json["fire_count"], Value::from(1));
    assert_eq!(json["last_job_id"], Value::from(job_id.as_str()));
    assert_eq!(json["last_run_id"], Value::from(job.run_id.clone()));
    assert!(json["last_fired_at"].is_string());

    // Unknown schedule id is a click-style error.
    let out = stado(&storage, &["schedule", "run", "sch-nope0000"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("Error: schedule sch-nope0000 not found"),
        "{}",
        stderr(&out)
    );
}

/// Library-level firing test: due schedules fire jobs tagged with
/// schedule_id; overlap_policy=skip suppresses a second fire while the
/// prior instance is still queued (but still advances next_due_at).
///
/// This is the ONLY test in this binary that resolves the config LazyLocks
/// in-process; it must set the env before the first `JobStorage::new()`.
#[tokio::test]
async fn fire_due_schedules_fires_and_overlap_skips() {
    let dir = tempfile::tempdir().unwrap();
    let storage = dir.path().join("storage");
    std::fs::create_dir_all(&storage).unwrap();
    std::env::set_var("WC_STORAGE_BACKEND", "local");
    std::env::set_var("WC_LOCAL_STORAGE_PATH", &storage);
    std::env::set_var("STADO_CONFIG", storage.join("no-such-config.json"));
    std::env::remove_var("COMPUTE_API_KEY");

    let store = JobStorage::new().await.unwrap();
    let now = Utc::now();
    let past = crate_iso(now - Duration::hours(1));

    // One due schedule (cron fires daily at 02:00 UTC).
    let mut sched = Schedule::new("sch-fire001", "0 2 * * *", "echo fired-by-schedule");
    sched.next_due_at = past.clone();
    schedules::write_schedule(&store, &sched).await.unwrap();

    // One future schedule that must NOT fire.
    let mut future = Schedule::new("sch-future1", "0 2 * * *", "echo not-yet");
    future.next_due_at = crate_iso(now + Duration::days(1));
    schedules::write_schedule(&store, &future).await.unwrap();

    let mut logs: Vec<String> = Vec::new();
    let fired = schedules::fire_due_schedules(&store, |m| logs.push(m.to_string()), now)
        .await
        .unwrap();
    assert_eq!(fired, 1, "{logs:?}");
    assert!(
        logs.iter()
            .any(|m| m.contains("fired job") && m.contains("sch-fire001")),
        "{logs:?}"
    );

    // The fired job is queued and tagged.
    let after = schedules::read_schedule(&store, "sch-fire001")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.fire_count, 1);
    assert!(!after.last_job_id.is_empty());
    let raw = std::fs::read_to_string(
        storage
            .join("queue")
            .join(format!("{}.json", after.last_job_id)),
    )
    .unwrap();
    let job = Job::from_json(&raw).unwrap();
    assert_eq!(job.schedule_id, "sch-fire001");
    assert!(job.run_id.starts_with("run-"));
    // next_due_at advanced to the next future 02:00 UTC.
    let next_due = after.next_due_at.clone();
    assert!(next_due > crate_iso(now), "{next_due}");

    // Force the schedule due again while the prior job is still queued:
    // overlap_policy=skip suppresses the fire but advances next_due_at.
    let mut again = after.clone();
    again.next_due_at = past;
    schedules::write_schedule(&store, &again).await.unwrap();
    let mut logs: Vec<String> = Vec::new();
    let fired = schedules::fire_due_schedules(&store, |m| logs.push(m.to_string()), now)
        .await
        .unwrap();
    assert_eq!(fired, 0, "{logs:?}");
    assert!(
        logs.iter()
            .any(|m| m.contains("skip fire") && m.contains(&after.last_job_id)),
        "{logs:?}"
    );
    let after2 = schedules::read_schedule(&store, "sch-fire001")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        after2.fire_count, 1,
        "overlap skip must not fire a second job"
    );
    assert!(
        after2.next_due_at > crate_iso(now),
        "next_due_at must still advance"
    );
    assert_eq!(
        std::fs::read_dir(storage.join("queue")).unwrap().count(),
        1,
        "still exactly one queued job"
    );

    // overlap_policy=allow ignores the live prior instance.
    let mut allow = after2.clone();
    allow.overlap_policy = "allow".into();
    allow.next_due_at = crate_iso(now - Duration::hours(1));
    schedules::write_schedule(&store, &allow).await.unwrap();
    let fired = schedules::fire_due_schedules(&store, |_| {}, now)
        .await
        .unwrap();
    assert_eq!(fired, 1);
    assert_eq!(std::fs::read_dir(storage.join("queue")).unwrap().count(), 2);

    // Disabled schedules never fire.
    let mut disabled = Schedule::new("sch-disabled", "* * * * *", "echo never");
    disabled.enabled = false;
    disabled.next_due_at = crate_iso(now - Duration::hours(1));
    schedules::write_schedule(&store, &disabled).await.unwrap();
    let before = std::fs::read_dir(storage.join("queue")).unwrap().count();
    let fired = schedules::fire_due_schedules(&store, |_| {}, now)
        .await
        .unwrap();
    assert_eq!(fired, 0);
    assert_eq!(
        std::fs::read_dir(storage.join("queue")).unwrap().count(),
        before
    );
}

/// `models::isoformat_utc` is crate-private; mirror the shape here (second
/// precision is enough for these comparisons).
fn crate_iso(dt: chrono::DateTime<Utc>) -> String {
    dt.format("%Y-%m-%dT%H:%M:%S+00:00").to_string()
}
