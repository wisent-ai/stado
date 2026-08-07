//! End-to-end CLI tests against the local storage backend.
//!
//! Every test drives the built `stado` binary (`CARGO_BIN_EXE_stado`) with
//! WC_STORAGE_BACKEND=local + WC_LOCAL_STORAGE_PATH=<TempDir>, mirroring
//! the Python CLI's behavior on a device-local deployment. STADO_CONFIG is
//! pointed at a nonexistent path so the developer's real
//! ~/.stado/config.json can never leak into a test.

use std::path::Path;
use std::process::{Command, Output};

use stado::models::Job;
use stado::queue::submit::{submit_job, SubmitOptions};

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

/// Extract the `Job ID: <id>` line from submit output.
fn job_id_of(out: &Output) -> String {
    stdout(out)
        .lines()
        .find_map(|line| line.strip_prefix("Job ID: "))
        .expect("submit echoed a Job ID")
        .trim()
        .to_string()
}

#[test]
fn submit_status_cancel_flow() {
    let dir = tempfile::tempdir().unwrap();
    let storage = dir.path().join("storage");
    let storage = storage.as_path();

    // Submit a plain CPU job.
    let out = stado(
        storage,
        &["submit", "echo hello-from-cli-test", "--priority", "3"],
    );
    assert!(out.status.success(), "submit failed: {}", stderr(&out));
    let job_id = job_id_of(&out);
    assert!(job_id.chars().all(|c| c.is_ascii_hexdigit()) && job_id.len() == 8);
    assert!(
        stdout(&out).contains("submitted 1/1 jobs"),
        "{}",
        stdout(&out)
    );
    assert!(stdout(&out).contains("via Stado"), "{}", stdout(&out));
    assert!(stdout(&out).contains("priority=3"), "{}", stdout(&out));

    // Status (fast path, exact job id) shows it queued.
    let out = stado(storage, &["status", &job_id]);
    assert!(out.status.success(), "status failed: {}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains(&job_id), "{text}");
    assert!(text.contains("queue"), "{text}");
    assert!(text.contains("echo hello-from-cli-test"), "{text}");

    // Status (slow path, no filter) shows the queue count.
    let out = stado(storage, &["status"]);
    assert!(out.status.success());
    assert!(stdout(&out).contains("1 queued"), "{}", stdout(&out));

    // Cancellation is a durable terminal transition, not deletion.
    let out = stado(storage, &["cancel", &job_id]);
    assert!(out.status.success(), "cancel failed: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), format!("Cancelled {job_id}"));
    assert!(storage
        .join(format!("cancellations/{job_id}.json"))
        .exists());
    assert!(storage.join(format!("cancelled/{job_id}.json")).exists());
    assert!(!storage.join(format!("queue/{job_id}.json")).exists());

    // Both exact and aggregate status expose the terminal cancellation.
    let out = stado(storage, &["status", &job_id]);
    assert!(out.status.success());
    assert!(stdout(&out).contains("cancelled"), "{}", stdout(&out));
    let out = stado(storage, &["status"]);
    assert!(out.status.success());
    assert!(stdout(&out).contains("1 cancelled"), "{}", stdout(&out));
}

#[test]
fn yieldable_without_on_yield_fails_with_python_message() {
    let dir = tempfile::tempdir().unwrap();
    let storage = dir.path().join("storage");
    let out = stado(storage.as_path(), &["submit", "echo hi", "--yieldable"]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "expected exit 1: {}",
        stdout(&out)
    );
    assert!(
        stderr(&out).contains(
            "Error: --yieldable requires --on-yield '<command>': a yieldable job must \
             declare how it saves state and steps aside. There is no silent \
             kill-and-restart path."
        ),
        "stderr: {}",
        stderr(&out)
    );
}

#[test]
fn deprecated_activation_entrypoint_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let storage = dir.path().join("storage");
    let out = stado(
        storage.as_path(),
        &[
            "submit",
            "python -m wisent.scripts.activations.extract_and_upload --x",
        ],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("refusing deprecated foreground activation uploader"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn profiles_lists_bundled_profile() {
    let dir = tempfile::tempdir().unwrap();
    let storage = dir.path().join("storage");
    let out = stado(storage.as_path(), &["profiles"]);
    assert!(out.status.success(), "profiles failed: {}", stderr(&out));
    assert!(
        stdout(&out).contains("ai_toolkit_zimage"),
        "{}",
        stdout(&out)
    );

    // Dumping one profile prints its JSON.
    let out = stado(storage.as_path(), &["profiles", "ai_toolkit_zimage"]);
    assert!(out.status.success());
    assert!(
        stdout(&out).contains("\"gpu_type\": \"nvidia-l4\""),
        "{}",
        stdout(&out)
    );

    // Unknown profile exits 1 with the click-style error.
    let out = stado(storage.as_path(), &["profiles", "no-such-profile-xyz"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("profile 'no-such-profile-xyz' not found"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn submit_with_profile_applies_profile_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let storage = dir.path().join("storage");
    let out = stado(
        storage.as_path(),
        &["submit", "echo profiled", "--profile", "ai_toolkit_zimage"],
    );
    assert!(out.status.success(), "submit failed: {}", stderr(&out));
    assert!(
        stdout(&out).contains("Profile 'ai_toolkit_zimage' applied:"),
        "{}",
        stdout(&out)
    );
    let job_id = job_id_of(&out);
    // The profile's gpu_type/vram_gb/machine_type/apt landed on the job.
    let json =
        std::fs::read_to_string(storage.join("queue").join(format!("{job_id}.json"))).unwrap();
    let job = Job::from_json(&json).unwrap();
    assert_eq!(job.gpu_type, "nvidia-l4");
    assert_eq!(job.gpu_mem_gb, 22);
    assert_eq!(job.machine_type, "g2-standard-8");
    assert!(job.exclusive);
    assert_eq!(
        job.apt_packages,
        vec!["libgl1", "libglib2.0-0", "git-lfs", "build-essential"]
    );
    assert_eq!(job.repo, "https://github.com/ostris/ai-toolkit.git");
    assert_eq!(job.repo_ref, "b677cdb02666320f1b03c747f5037a41e5a7515e");
}

#[test]
fn config_validate_passes_on_temp_config() {
    let dir = tempfile::tempdir().unwrap();
    let storage = dir.path().join("storage");
    let config_path = dir.path().join("stado-config.json");
    std::fs::write(
        &config_path,
        serde_json::to_vec_pretty(&stado::config_file::template()).unwrap(),
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(["config", "validate"])
        .env("STADO_CONFIG", &config_path)
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", &storage)
        .output()
        .expect("stado binary runs");
    assert!(
        out.status.success(),
        "config validate failed: {}",
        stderr(&out)
    );
    assert!(stdout(&out).starts_with("config ok ("), "{}", stdout(&out));

    // A broken config reports ERROR lines and exits 1.
    std::fs::write(&config_path, r#"{"storage": {"backend": "ftp"}}"#).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(["config", "validate"])
        .env("STADO_CONFIG", &config_path)
        .output()
        .expect("stado binary runs");
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stdout(&out).contains("ERROR storage.backend must be gcs|azure|s3|local"),
        "{}",
        stdout(&out)
    );

    // Unknown subcommand is a click-style error.
    let out = stado(storage.as_path(), &["config", "bogus"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("unknown config subcommand: bogus (show|validate|init|migrate)"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn weles_recordings_dir_rejects_relative_path_before_storage_access() {
    let dir = tempfile::tempdir().unwrap();
    let storage = dir.path().join("storage");
    let out = stado(
        storage.as_path(),
        &["host", "weles-recordings-dir", "somehost", "relative/rec"],
    );
    assert_eq!(out.status.code(), Some(i32::from(true)));
    assert!(
        stderr(&out).contains("PATH must be absolute"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn coordinator_unknown_target_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    let storage = dir.path().join("storage");
    // --target resolution fails before any storage/network access.
    let out = stado(
        storage.as_path(),
        &["coordinator", "--target", "no-such-coord", "--once"],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("coordinator selector 'no-such-coord' not found in registry"),
        "{}",
        stderr(&out)
    );
}

/// Library-level test: `submit_job` writes `queue/<id>.json` whose content
/// `Job::from_json`-round-trips byte-identically and whose fields match the
/// requested flags.
#[tokio::test]
async fn submit_job_writes_roundtrippable_queue_blob() {
    let dir = tempfile::tempdir().unwrap();
    let storage = dir.path().join("storage");
    // First in-process touch of the config LazyLocks: pin the local backend.
    std::env::set_var("WC_STORAGE_BACKEND", "local");
    std::env::set_var("WC_LOCAL_STORAGE_PATH", &storage);
    std::env::set_var("STADO_CONFIG", storage.join("no-such-config.json"));
    std::env::remove_var("COMPUTE_API_KEY");

    let options = SubmitOptions {
        priority: 7,
        vram_gb: 22,
        provider: "gcp".into(),
        preemptible: true,
        repo: "https://github.com/org/repo.git".into(),
        repo_ref: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
        apt_packages: vec!["htop".into(), "git-lfs".into()],
        pre_command: "export FOO=bar".into(),
        yieldable: true,
        yield_command: "touch /tmp/yield".into(),
        yield_grace_seconds: 60,
        batch_id: "batch-test".into(),
        ..Default::default()
    };
    let job = submit_job("python -m train --epochs 1", &options)
        .await
        .expect("submit_job succeeds");

    // The blob landed on disk and round-trips byte-identically.
    let blob = storage.join("queue").join(format!("{}.json", job.job_id));
    let raw = std::fs::read_to_string(&blob).expect("queue blob written");
    let parsed = Job::from_json(&raw).expect("blob parses");
    assert_eq!(
        parsed.to_json(),
        raw,
        "blob must round-trip byte-identically"
    );

    // Fields match the requested flags.
    assert_eq!(parsed.job_id, job.job_id);
    assert_eq!(parsed.command, "python -m train --epochs 1");
    assert_eq!(parsed.state, "queued");
    assert_eq!(parsed.priority, 7);
    assert!(parsed.preemptible);
    assert_eq!(parsed.gpu_mem_gb, 22);
    // vram_gb=22 on gcp resolves through lookup_instance_type to the L4 tier.
    assert_eq!(parsed.gpu_type, "nvidia-l4");
    assert_eq!(parsed.machine_type, "g2-standard-4");
    assert_eq!(parsed.repo, "https://github.com/org/repo.git");
    assert_eq!(parsed.repo_workdir, ""); // default = repo basename, resolved in the script only
    assert_eq!(parsed.repo_extras, "train");
    assert_eq!(parsed.apt_packages, vec!["htop", "git-lfs"]);
    assert_eq!(parsed.pre_command, "export FOO=bar");
    assert!(parsed.yieldable);
    assert_eq!(parsed.yield_command, "touch /tmp/yield");
    assert_eq!(parsed.yield_grace_seconds, 60);
    assert_eq!(parsed.batch_id, "batch-test");
    assert_eq!(parsed.submitted_via, "cli");
}
