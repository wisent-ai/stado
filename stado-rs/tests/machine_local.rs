//! End-to-end `stado machine ...` tests against the local storage backend.
//!
//! Drives the built `stado` binary (`CARGO_BIN_EXE_stado`) with
//! WC_STORAGE_BACKEND=local + WC_LOCAL_STORAGE_PATH=<TempDir>, and checks
//! the versioned JSON envelope on stdout (schema_version/ok/result|error),
//! idempotent submit semantics, byte-cursor log paging, the durable cancel
//! marker, and artifact download with sha256 verification.

use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;
use stado::models::Job;

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

/// Parse the single envelope line; every machine command must emit exactly
/// one JSON object on stdout.
fn envelope(out: &Output) -> Value {
    let text = stdout(out);
    let parsed: Value = serde_json::from_str(text.trim())
        .unwrap_or_else(|exc| panic!("stdout is not one JSON envelope: {exc}\n{text}"));
    assert_eq!(parsed["schema_version"], Value::from(1), "{text}");
    parsed
}

fn ok_envelope(out: &Output) -> Value {
    let env = envelope(out);
    assert!(out.status.success(), "expected exit 0: {}", stderr(out));
    assert_eq!(env["ok"], Value::from(true), "{env}");
    env["result"].clone()
}

fn err_envelope(out: &Output) -> Value {
    let env = envelope(out);
    assert_eq!(out.status.code(), Some(1), "expected exit 1: {env}");
    assert_eq!(env["ok"], Value::from(false), "{env}");
    env["error"].clone()
}

/// Write a machine request JSON file into `dir` and return its path.
fn request_file(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    path
}

/// Plant a job blob directly in the local storage layout.
fn plant_job(storage: &Path, prefix: &str, job_id: &str) -> Job {
    let job = Job::new(job_id, "echo planted");
    std::fs::create_dir_all(storage.join(prefix)).unwrap();
    std::fs::write(storage.join(prefix).join(format!("{job_id}.json")), job.to_json()).unwrap();
    job
}

#[test]
fn machine_submit_is_idempotent_and_conflicts_on_digest_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let storage = dir.path().join("storage");
    std::fs::create_dir_all(&storage).unwrap();
    let request = request_file(
        dir.path(),
        "request.json",
        r#"{"client_request_id": "req-alpha-1", "command": "echo machine-test", "priority": 2}"#,
    );
    let request_arg = request.to_str().unwrap();

    let out = stado(&storage, &["machine", "submit", "--request-file", request_arg]);
    let first = ok_envelope(&out);
    let job_id = first["job"]["job_id"].as_str().expect("job_id in result").to_string();
    assert_eq!(job_id.len(), 8);
    assert_eq!(first["job"]["state"], Value::from("queued"));
    assert_eq!(first["job"]["command"], Value::from("echo machine-test"));
    assert_eq!(first["job"]["error"], Value::Null);
    assert_eq!(first["job"]["started_at"], Value::Null);

    // The reservation record landed and the job is queued.
    assert!(storage.join("machine_requests/req-alpha-1.json").exists());
    assert!(storage.join(format!("queue/{job_id}.json")).exists());

    // Exact replay returns the STORED result: same job, no new submission.
    let out = stado(&storage, &["machine", "submit", "--request-file", request_arg]);
    let replay = ok_envelope(&out);
    assert_eq!(replay, first, "replay must return the stored result verbatim");
    assert_eq!(
        std::fs::read_dir(storage.join("queue")).unwrap().count(),
        1,
        "replay must not submit a second job"
    );

    // Same client_request_id with a changed payload: IDEMPOTENCY_CONFLICT.
    let changed = request_file(
        dir.path(),
        "changed.json",
        r#"{"client_request_id": "req-alpha-1", "command": "echo DIFFERENT"}"#,
    );
    let out =
        stado(&storage, &["machine", "submit", "--request-file", changed.to_str().unwrap()]);
    let err = err_envelope(&out);
    assert_eq!(err["code"], Value::from("IDEMPOTENCY_CONFLICT"), "{err}");
    assert_eq!(err["retryable"], Value::from(false));
}

#[test]
fn machine_submit_validates_request_shape() {
    let dir = tempfile::tempdir().unwrap();
    let storage = dir.path().join("storage");
    std::fs::create_dir_all(&storage).unwrap();

    // Unknown field.
    let bad = request_file(
        dir.path(),
        "unknown.json",
        r#"{"client_request_id": "r1", "command": "x", "surprise": true}"#,
    );
    let out = stado(&storage, &["machine", "submit", "--request-file", bad.to_str().unwrap()]);
    let err = err_envelope(&out);
    assert_eq!(err["code"], Value::from("INVALID_REQUEST"), "{err}");
    assert!(err["message"].as_str().unwrap().contains("unknown request field(s): surprise"), "{err}");

    // Path-unsafe client_request_id.
    let bad = request_file(
        dir.path(),
        "badid.json",
        r#"{"client_request_id": "a/b", "command": "x"}"#,
    );
    let out = stado(&storage, &["machine", "submit", "--request-file", bad.to_str().unwrap()]);
    let err = err_envelope(&out);
    assert_eq!(err["code"], Value::from("INVALID_REQUEST"), "{err}");

    // Unreadable request file.
    let out = stado(
        &storage,
        &["machine", "submit", "--request-file", dir.path().join("nope.json").to_str().unwrap()],
    );
    let err = err_envelope(&out);
    assert_eq!(err["code"], Value::from("INVALID_REQUEST"), "{err}");
    assert!(err["message"].as_str().unwrap().contains("cannot read request JSON"), "{err}");

    // Source archives are gated to the GCS backend.
    let archive = dir.path().join("src.tar.gz");
    std::fs::write(&archive, b"placeholder").unwrap();
    let bad = request_file(
        dir.path(),
        "archive.json",
        &format!(
            r#"{{"client_request_id": "r2", "command": "x", "source_archive_path": "{}"}}"#,
            archive.display()
        ),
    );
    let out = stado(&storage, &["machine", "submit", "--request-file", bad.to_str().unwrap()]);
    let err = err_envelope(&out);
    // The placeholder is not a tar.gz: validation fails before the backend gate.
    assert_eq!(err["code"], Value::from("INVALID_SOURCE_ARCHIVE"), "{err}");
}

#[test]
fn machine_status_and_logs_cursor_paging() {
    let dir = tempfile::tempdir().unwrap();
    let storage = dir.path().join("storage");
    std::fs::create_dir_all(&storage).unwrap();
    plant_job(&storage, "queue", "aa11bb22");

    let out = stado(&storage, &["machine", "status", "aa11bb22"]);
    let result = ok_envelope(&out);
    assert_eq!(result["job"]["job_id"], Value::from("aa11bb22"));
    assert_eq!(result["job"]["state"], Value::from("queued"));

    let out = stado(&storage, &["machine", "status", "ffffffff"]);
    let err = err_envelope(&out);
    assert_eq!(err["code"], Value::from("NOT_FOUND"), "{err}");
    assert_eq!(err["message"], Value::from("job 'ffffffff' was not found"), "{err}");

    // Plant a command log and page through it by byte cursor.
    let output = storage.join("status/aa11bb22/output");
    std::fs::create_dir_all(&output).unwrap();
    std::fs::write(output.join("command_output.log"), b"0123456789").unwrap();

    let out = stado(&storage, &["machine", "logs", "aa11bb22", "--cursor", "0", "--limit", "4"]);
    let page = ok_envelope(&out);
    assert_eq!(page["text"], Value::from("0123"));
    assert_eq!(page["cursor"], Value::from(0));
    assert_eq!(page["next_cursor"], Value::from(4));
    assert_eq!(page["eof"], Value::from(false));

    let out = stado(&storage, &["machine", "logs", "aa11bb22", "--cursor", "4", "--limit", "100"]);
    let page = ok_envelope(&out);
    assert_eq!(page["text"], Value::from("456789"));
    assert_eq!(page["next_cursor"], Value::from(10));
    assert_eq!(page["eof"], Value::from(true));

    // Cursor beyond the end is INVALID_CURSOR.
    let out = stado(&storage, &["machine", "logs", "aa11bb22", "--cursor", "11", "--limit", "5"]);
    let err = err_envelope(&out);
    assert_eq!(err["code"], Value::from("INVALID_CURSOR"), "{err}");
    // Negative cursor and non-positive limit are INVALID_CURSOR too.
    let out = stado(&storage, &["machine", "logs", "aa11bb22", "--cursor", "-1"]);
    assert_eq!(err_envelope(&out)["code"], Value::from("INVALID_CURSOR"));
    let out = stado(&storage, &["machine", "logs", "aa11bb22", "--limit", "0"]);
    assert_eq!(err_envelope(&out)["code"], Value::from("INVALID_CURSOR"));
}

#[test]
fn machine_cancel_is_durable_and_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let storage = dir.path().join("storage");
    std::fs::create_dir_all(&storage).unwrap();
    plant_job(&storage, "queue", "cancel99");

    let out = stado(&storage, &["machine", "cancel", "cancel99"]);
    let result = ok_envelope(&out);
    assert_eq!(result["job"]["state"], Value::from("cancelled"));
    assert_eq!(result["job"]["error"], Value::from("cancelled"));

    // Durable marker + prefix transition.
    let marker = storage.join("cancellations/cancel99.json");
    assert!(marker.exists(), "cancellations/ marker must be durable");
    let marker_json: Value =
        serde_json::from_str(&std::fs::read_to_string(marker).unwrap()).unwrap();
    assert_eq!(marker_json["job_id"], Value::from("cancel99"));
    assert!(marker_json["requested_at"].is_string());
    assert!(!storage.join("queue/cancel99.json").exists());
    assert!(storage.join("cancelled/cancel99.json").exists());

    // Cancelling the terminal job again is a no-op success.
    let out = stado(&storage, &["machine", "cancel", "cancel99"]);
    let result = ok_envelope(&out);
    assert_eq!(result["job"]["state"], Value::from("cancelled"));
}

#[test]
fn machine_artifacts_download_verifies_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let storage = dir.path().join("storage");
    std::fs::create_dir_all(&storage).unwrap();
    plant_job(&storage, "completed", "artjob77");
    let output = storage.join("status/artjob77/output");
    std::fs::create_dir_all(output.join("nested")).unwrap();
    std::fs::write(output.join("metrics.json"), b"{\"loss\": 0.1}").unwrap();
    std::fs::write(output.join("nested/blob.bin"), b"\x00\x01\x02").unwrap();

    let out_dir = dir.path().join("download");
    let out = stado(
        &storage,
        &["machine", "artifacts", "artjob77", "--output-dir", out_dir.to_str().unwrap()],
    );
    let result = ok_envelope(&out);
    assert_eq!(result["job_id"], Value::from("artjob77"));
    let artifacts = result["artifacts"].as_array().unwrap();
    assert_eq!(artifacts.len(), 2, "{result}");
    assert_eq!(artifacts[0]["relative_path"], Value::from("metrics.json"));
    assert_eq!(artifacts[0]["size_bytes"], Value::from(13));
    use sha2::Digest;
    assert_eq!(
        artifacts[0]["sha256"],
        Value::from(hex::encode(sha2::Sha256::digest(b"{\"loss\": 0.1}")))
    );
    assert_eq!(artifacts[1]["relative_path"], Value::from("nested/blob.bin"));
    // Files landed under the output dir, no tempfiles left behind.
    assert_eq!(std::fs::read(out_dir.join("metrics.json")).unwrap(), b"{\"loss\": 0.1}");
    assert_eq!(std::fs::read(out_dir.join("nested/blob.bin")).unwrap(), b"\x00\x01\x02");
    assert!(!std::fs::read_dir(&out_dir).unwrap().any(|e| {
        e.unwrap().file_name().to_string_lossy().starts_with(".stado-")
    }));

    // Non-terminal job: NOT_TERMINAL.
    plant_job(&storage, "running", "running1");
    let out = stado(
        &storage,
        &["machine", "artifacts", "running1", "--output-dir", out_dir.to_str().unwrap()],
    );
    let err = err_envelope(&out);
    assert_eq!(err["code"], Value::from("NOT_TERMINAL"), "{err}");

    // Terminal job with no canonical output: NO_ARTIFACTS.
    plant_job(&storage, "failed", "emptyj1");
    let out = stado(
        &storage,
        &["machine", "artifacts", "emptyj1", "--output-dir", out_dir.to_str().unwrap()],
    );
    let err = err_envelope(&out);
    assert_eq!(err["code"], Value::from("NO_ARTIFACTS"), "{err}");
}
