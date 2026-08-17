//! Real user-workflow integration scenarios.
//!
//! Local filesystems stand in for object stores so these scenarios remain
//! deterministic and credential-free. The CLI, queue, agent slots, storage
//! copier, machine facade, HTTP secret boundary, state transitions, and
//! artifact downloads are production implementations.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::num::NonZeroU64;
use std::path::Path;
use std::process::{Command, Output};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;
use sha2::{Digest, Sha256};
use stado::models::Job;
use stado::providers::gcp::GcpProvider;
use stado::providers::local::agent::POLL_INTERVAL_S;
use stado::providers::local::slots::{self, ActiveSlot, SlotOutcome};
use stado::providers::Provider;
use stado::queue::{
    AzureBlobBackend, BlobBackend, GcsBackend, JobStorage, LocalBackend, S3Backend,
};
use stado::sizing::Sizing;

const WORKLOAD_SECRET: &str = "model-token-visible-only-to-the-workload";

fn stado(storage: &Path, args: &[&str]) -> Output {
    stado_with_env(storage, args, &[])
}

fn stado_with_env(storage: &Path, args: &[&str], extra_env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
    command
        .args(args)
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", storage)
        .env("STADO_CONFIG", storage.join("no-such-config.json"))
        .env_remove("COMPUTE_API_KEY")
        .env_remove("COMPUTE_API_URL")
        .env_remove("STADO_API_TOKEN")
        .env_remove("STADO_API_URL")
        .env_remove("WC_PROFILES_DIR")
        .env_remove("WC_AGENT_SKARBIEC_URL")
        .env_remove("WC_AGENT_SKARBIEC_CONSUMER")
        .env_remove("WC_AGENT_SKARBIEC_TOKEN_FILE")
        .env_remove("WC_AGENT_SKARBIEC_SECRET_FIELDS");
    for (name, value) in extra_env {
        command.env(name, value);
    }
    command.output().expect("stado binary runs")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn assert_success(output: &Output, action: &str) {
    assert!(
        output.status.success(),
        "{action} failed\nstdout:\n{}\nstderr:\n{}",
        stdout(output),
        stderr(output)
    );
}

fn submitted_job_id(output: &Output) -> String {
    stdout(output)
        .lines()
        .find_map(|line| line.strip_prefix("Job ID: "))
        .expect("submit output contains a Job ID")
        .trim()
        .to_string()
}

fn machine_result(output: &Output) -> Value {
    assert_success(output, "machine command");
    let envelope: Value = serde_json::from_str(stdout(output).trim())
        .unwrap_or_else(|error| panic!("machine stdout is not JSON: {error}\n{}", stdout(output)));
    assert_eq!(envelope["schema_version"], Value::from(u8::from(true)));
    assert_eq!(envelope["ok"], Value::from(true));
    envelope["result"].clone()
}

fn parse_json_stdout(output: &Output, action: &str) -> Value {
    assert_success(output, action);
    serde_json::from_str(stdout(output).trim())
        .unwrap_or_else(|error| panic!("{action} did not return JSON: {error}\n{}", stdout(output)))
}

fn json_count<T>(items: &[T]) -> Value {
    Value::from(u64::try_from(items.len()).expect("scenario count fits u64"))
}

fn json_zero() -> Value {
    Value::from(u64::MIN)
}

fn local_store(storage: &Path) -> JobStorage {
    let backend = LocalBackend::new(storage.to_str().expect("UTF-8 temp path"))
        .expect("local backend is created");
    JobStorage::with_backend(Arc::new(backend), "local")
}

fn queue_blob(storage: &Path, job_id: &str) -> Vec<u8> {
    std::fs::read(storage.join("queue").join(format!("{job_id}.json")))
        .expect("queued job blob exists")
}

struct WorkdirGuard {
    job_id: String,
}
struct EnvGuard {
    previous: Vec<(String, Option<std::ffi::OsString>)>,
}

impl EnvGuard {
    fn set(values: &[(&str, &str)]) -> Self {
        let previous = values
            .iter()
            .map(|(name, value)| {
                let old = std::env::var_os(name);
                std::env::set_var(name, value);
                ((*name).to_string(), old)
            })
            .collect();
        Self { previous }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (name, value) in self.previous.drain(..) {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }
}

impl WorkdirGuard {
    fn new(job_id: &str) -> Self {
        Self {
            job_id: job_id.to_string(),
        }
    }
}

impl Drop for WorkdirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(format!("/tmp/wc-{}", self.job_id));
    }
}

async fn advance_to_done(mut slot: ActiveSlot, store: &JobStorage, log: &mut dyn FnMut(&str)) {
    let sizing = Sizing::new();
    let deadline = Instant::now() + Duration::from_secs(POLL_INTERVAL_S);
    loop {
        assert!(
            Instant::now() < deadline,
            "job {} did not finish before the scenario deadline",
            slot.slot.job.job_id
        );
        match slots::advance_slot(slot, store, &sizing, false, log)
            .await
            .expect("agent advances the slot")
        {
            SlotOutcome::Running(next) => {
                slot = next;
                tokio::task::yield_now().await;
            }
            SlotOutcome::Done => return,
        }
    }
}

fn request_is_complete(bytes: &[u8]) -> bool {
    let marker = b"\r\n\r\n";
    let Some(header_end) = bytes
        .windows(marker.len())
        .position(|window| window == marker)
    else {
        return false;
    };
    let headers = String::from_utf8_lossy(&bytes[..header_end]);
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or_default();
    bytes.len() >= header_end + marker.len() + content_length
}

fn read_request(stream: &mut TcpStream) -> String {
    let timeout_seconds = NonZeroU64::MIN.get() + NonZeroU64::MIN.get();
    stream
        .set_read_timeout(Some(Duration::from_secs(timeout_seconds)))
        .expect("mock read timeout is configured");
    let mut request = Vec::new();
    let mut chunk = [u8::MIN; u8::MAX as usize];
    loop {
        match stream.read(&mut chunk) {
            Ok(read) if read == usize::MIN => break,
            Ok(read) => {
                request.extend_from_slice(&chunk[..read]);
                if request_is_complete(&request) {
                    break;
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                break;
            }
            Err(error) => panic!("mock Skarbiec request read failed: {error}"),
        }
    }
    String::from_utf8(request).expect("Skarbiec request is UTF-8 HTTP")
}

fn spawn_skarbiec() -> (String, thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("mock Skarbiec binds");
    listener
        .set_nonblocking(true)
        .expect("mock listener becomes nonblocking");
    let address = listener.local_addr().expect("mock listener address");
    let handle = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(POLL_INTERVAL_S);
        let expected_requests = usize::try_from(u16::BITS / u8::BITS).unwrap();
        let mut requests = Vec::with_capacity(expected_requests);
        while requests.len() < expected_requests {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream
                        .set_nonblocking(false)
                        .expect("accepted Skarbiec connection becomes blocking");
                    let request = read_request(&mut stream);

                    let body = format!(r#"{{"value":"{WORKLOAD_SECRET}"}}"#);
                    write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    )
                    .expect("mock Skarbiec response is written");
                    requests.push(request);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "agent did not make every expected Skarbiec request"
                    );
                    thread::yield_now();
                }
                Err(error) => panic!("mock Skarbiec accept failed: {error}"),
            }
        }
        requests.join("\n")
    });
    (format!("http://{address}"), handle)
}

fn assert_tree_omits(root: &Path, needle: &[u8]) {
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        for entry in std::fs::read_dir(&path).expect("durable state directory is readable") {
            let entry = entry.expect("durable state entry is readable");
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            let content = std::fs::read(&path).expect("durable state file is readable");
            assert!(
                !content.windows(needle.len()).any(|window| window == needle),
                "secret leaked into durable state at {}",
                path.display()
            );
        }
    }
}

/// Researcher journey: human CLI submission, real shell execution, a
/// post-command verification hook, canonical terminal state, and result pull.
#[tokio::test]
async fn researcher_submits_executes_verifies_and_downloads_local_training_result() {
    let temp = tempfile::tempdir().unwrap();
    let storage = temp.path().join("storage");
    let command = r#"mkdir -p output && printf '%s\n' '{"loss":0.125,"steps":3}' > output/metrics.json && printf 'epoch 3 complete\n'"#;

    let submitted = stado(
        &storage,
        &[
            "submit",
            command,
            "--priority",
            "7",
            "--verify",
            "test -s output/metrics.json",
        ],
    );
    assert_success(&submitted, "researcher submit");
    let job_id = submitted_job_id(&submitted);
    let _workdir = WorkdirGuard::new(&job_id);
    assert!(stdout(&submitted).contains("priority=7"));

    let store = local_store(&storage);
    let queued = store
        .read_job("queue", &job_id)
        .await
        .expect("queue is readable")
        .expect("submitted job is queued");
    assert_eq!(queued.verify_command, "test -s output/metrics.json");

    let mut agent_log = Vec::new();
    let mut logger = |line: &str| agent_log.push(line.to_string());
    let slot = slots::start_slot(
        &store,
        queued,
        "researcher-workstation",
        &mut logger,
        "local",
        None,
    )
    .await
    .expect("agent starts submitted job")
    .expect("job is admitted");
    advance_to_done(slot, &store, &mut logger).await;

    assert!(store.read_job("queue", &job_id).await.unwrap().is_none());
    assert!(store.read_job("running", &job_id).await.unwrap().is_none());
    let completed = store
        .read_job("completed", &job_id)
        .await
        .unwrap()
        .expect("job reaches completed state");
    assert_eq!(completed.state, "completed");
    assert!(completed.completed_at.is_some());
    assert!(completed.error.is_none());
    assert_eq!(
        store
            .download_text(&format!("status/{job_id}/status"))
            .await
            .unwrap()
            .as_deref(),
        Some("COMPLETED")
    );

    let status = stado(&storage, &["status", &job_id]);
    assert_success(&status, "researcher status");
    assert!(stdout(&status).contains(&job_id));
    assert!(stdout(&status).contains("completed"));

    let download = temp.path().join("downloaded-results");
    let results = stado(&storage, &["results", &job_id, download.to_str().unwrap()]);
    assert_success(&results, "researcher result download");
    assert_eq!(
        std::fs::read_to_string(download.join("metrics.json")).unwrap(),
        "{\"loss\":0.125,\"steps\":3}\n"
    );
    assert!(std::fs::read_to_string(download.join("command_output.log"))
        .unwrap()
        .contains("epoch 3 complete"));
    assert!(agent_log
        .iter()
        .any(|line| line.contains(&format!("Job {job_id} completed"))));
}

/// Maintenance journey: stop admissions, drain active work, preserve queued
/// jobs byte-for-byte, and explicitly reopen dispatch.
#[test]
fn operator_maintenance_pause_preserves_queued_work_and_resume_reopens_dispatch() {
    let temp = tempfile::tempdir().unwrap();
    let storage = temp.path().join("storage");

    let first = stado(&storage, &["submit", "echo first-training-job"]);
    let second = stado(&storage, &["submit", "echo second-training-job"]);
    assert_success(&first, "first submit");
    assert_success(&second, "second submit");
    let first_id = submitted_job_id(&first);
    let second_id = submitted_job_id(&second);
    let submitted_jobs = [&first_id, &second_id];
    let first_before = queue_blob(&storage, &first_id);
    let second_before = queue_blob(&storage, &second_id);

    let paused = stado(
        &storage,
        &["queue", "pause", "--reason", "planned GPU maintenance"],
    );
    assert_success(&paused, "queue pause");
    assert!(stdout(&paused).contains("planned GPU maintenance"));

    let paused_status = parse_json_stdout(
        &stado(&storage, &["queue", "status", "--json"]),
        "paused queue status",
    );
    assert_eq!(paused_status["paused"], Value::from(true));
    assert_eq!(
        paused_status["counts"]["queue"],
        json_count(&submitted_jobs)
    );
    assert_eq!(paused_status["counts"]["running"], json_zero());

    let drained = stado(&storage, &["queue", "drain", "--wait", "--timeout", "1"]);
    assert_success(&drained, "queue drain");
    assert!(stdout(&drained).contains("running/ is empty"));
    assert!(stdout(&drained).contains("2 job(s) remain queued and untouched"));
    assert_eq!(queue_blob(&storage, &first_id), first_before);
    assert_eq!(queue_blob(&storage, &second_id), second_before);

    let resumed = stado(&storage, &["queue", "resume"]);
    assert_success(&resumed, "queue resume");
    let resumed_status = parse_json_stdout(
        &stado(&storage, &["queue", "status", "--json"]),
        "resumed queue status",
    );
    assert_eq!(resumed_status["paused"], Value::from(false));
    assert_eq!(
        resumed_status["counts"]["queue"],
        json_count(&submitted_jobs)
    );
    assert_eq!(queue_blob(&storage, &first_id), first_before);
    assert_eq!(queue_blob(&storage, &second_id), second_before);
}

/// Outage journey: preview a canonical copy, perform it, verify names,
/// metadata, and body bytes, then read the recovered queue from its new store.
#[test]
fn operator_migrates_canonical_state_after_storage_outage_and_verifies_cutover() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source-store");
    let destination = temp.path().join("destination-store");

    let first = stado(&source, &["submit", "echo queued-before-outage"]);
    let second = stado(&source, &["submit", "echo another-queued-job"]);
    assert_success(&first, "first pre-outage submit");
    assert_success(&second, "second pre-outage submit");
    let first_id = submitted_job_id(&first);
    let second_id = submitted_job_id(&second);
    let recovered_jobs = [&first_id, &second_id];
    let source_evidence = source.join(format!("status/{first_id}/output/progress.json"));
    std::fs::create_dir_all(source_evidence.parent().unwrap()).unwrap();
    std::fs::write(&source_evidence, b"{\"epoch\":4}\n").unwrap();

    let source_path = source.to_str().unwrap();
    let destination_path = destination.to_str().unwrap();
    let locators = [
        "storage",
        "copy",
        "--from",
        "local",
        "--to",
        "local",
        "--from-path",
        source_path,
        "--to-path",
        destination_path,
    ];
    let mut dry_run_args = locators.to_vec();
    dry_run_args.push("--dry-run");
    let dry_run = stado(&source, &dry_run_args);
    assert_success(&dry_run, "storage migration dry-run");
    assert!(stdout(&dry_run).contains("DRY RUN — nothing is written"));
    assert!(!destination.join("queue").exists());

    let copied = stado(&source, &locators);
    assert_success(&copied, "canonical storage copy");
    assert!(stdout(&copied).contains("WARNING: copying a LIVE queue produces split-brain"));

    let verify_args = [
        "storage",
        "verify",
        "--from",
        "local",
        "--to",
        "local",
        "--from-path",
        source_path,
        "--to-path",
        destination_path,
        "--json",
    ];
    let verified = parse_json_stdout(&stado(&source, &verify_args), "post-copy verification");
    assert_eq!(verified["divergent"], Value::from(false));
    assert_eq!(verified["missing_at_destination"], json_zero());
    assert_eq!(verified["body_mismatches"], json_zero());
    assert_eq!(verified["metadata_mismatches"], json_zero());

    assert_eq!(
        queue_blob(&destination, &first_id),
        queue_blob(&source, &first_id)
    );
    assert_eq!(
        queue_blob(&destination, &second_id),
        queue_blob(&source, &second_id)
    );
    assert_eq!(
        std::fs::read(destination.join(format!("status/{first_id}/output/progress.json"))).unwrap(),
        b"{\"epoch\":4}\n"
    );

    let destination_status = parse_json_stdout(
        &stado(&destination, &["queue", "status", "--json"]),
        "destination queue read",
    );
    assert_eq!(
        destination_status["counts"]["queue"],
        json_count(&recovered_jobs)
    );
}

/// AI-agent journey: submit a machine-readable request with a digest-pinned
/// input and a field-scoped secret; materialize both only at execution time;
/// prove the secret never entered durable queue or result state.
#[tokio::test]
async fn ai_agent_request_materializes_pinned_input_and_scoped_secret_without_leakage() {
    let temp = tempfile::tempdir().unwrap();
    let storage = temp.path().join("storage");
    let input = temp.path().join("training.csv");
    let input_bytes = b"sample,score\nalpha,1\n";
    std::fs::write(&input, input_bytes).unwrap();
    let input_sha = hex::encode(Sha256::digest(input_bytes));

    let put = stado(
        &storage,
        &[
            "storage",
            "put",
            "stado://datasets/training.csv",
            input.to_str().unwrap(),
            "--if-absent",
        ],
    );
    assert_success(&put, "immutable input upload");

    let command = "mkdir -p output && test -s inputs/training.csv && test -n \"$MODEL_TOKEN\" && /usr/bin/wc -l < inputs/training.csv | /usr/bin/tr -d ' ' > output/rows.txt && printf '%s' \"$MODEL_TOKEN\" | /usr/bin/wc -c | /usr/bin/tr -d ' ' > output/secret-length.txt";
    let request = serde_json::json!({
        "client_request_id": "agent-training-request-1",
        "command": command,
        "input_objects": {
            "training_data": {
                "stado_uri": "stado://datasets/training.csv",
                "relative_path": "inputs/training.csv",
                "sha256": input_sha.clone(),
            }
        },
        "secret_env": {
            "MODEL_TOKEN": {
                "item": "model-provider",
                "field": "token",
            }
        }
    });
    let request_path = temp.path().join("machine-request.json");
    std::fs::write(&request_path, serde_json::to_vec_pretty(&request).unwrap()).unwrap();
    let secret_fields = "model-provider#token";
    let submitted = stado_with_env(
        &storage,
        &[
            "machine",
            "submit",
            "--request-file",
            request_path.to_str().unwrap(),
        ],
        &[("WC_AGENT_SKARBIEC_SECRET_FIELDS", secret_fields)],
    );
    let result = machine_result(&submitted);
    let job_id = result["job"]["job_id"]
        .as_str()
        .expect("machine result contains job id")
        .to_string();
    let _workdir = WorkdirGuard::new(&job_id);
    assert_eq!(result["job"]["state"], Value::from("queued"));

    let queued_raw = queue_blob(&storage, &job_id);
    assert!(!queued_raw
        .windows(WORKLOAD_SECRET.len())
        .any(|window| window == WORKLOAD_SECRET.as_bytes()));
    let queued = Job::from_json(std::str::from_utf8(&queued_raw).unwrap()).unwrap();
    assert_eq!(queued.secret_env["MODEL_TOKEN"].item, "model-provider");
    assert_eq!(queued.secret_env["MODEL_TOKEN"].field, "token");
    assert_eq!(
        queued.resolved_input_artifacts["training_data"]["sha256"],
        Value::from(input_sha)
    );

    let token_file = temp.path().join("workload-agent.grant");
    std::fs::write(&token_file, "test-agent-grant\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let owner_only = u32::from_str_radix("600", u8::BITS).unwrap();
        std::fs::set_permissions(&token_file, std::fs::Permissions::from_mode(owner_only)).unwrap();
    }
    let (skarbiec_url, skarbiec_request) = spawn_skarbiec();
    std::env::set_var(
        "STADO_CONFIG",
        temp.path().join("no-such-parent-config.json"),
    );
    std::env::set_var("WC_AGENT_SKARBIEC_URL", &skarbiec_url);
    std::env::set_var("WC_AGENT_SKARBIEC_CONSUMER", "workflow-test-agent");
    std::env::set_var("WC_AGENT_SKARBIEC_TOKEN_FILE", &token_file);
    std::env::set_var("WC_AGENT_SKARBIEC_SECRET_FIELDS", secret_fields);

    let store = local_store(&storage);
    let mut agent_log = Vec::new();
    let mut logger = |line: &str| agent_log.push(line.to_string());
    let slot = slots::start_slot(&store, queued, "automation-worker", &mut logger, "local", None)
        .await
        .expect("trusted agent starts machine job");
    let slot = match slot {
        Some(slot) => slot,
        None => {
            drop(logger);
            panic!(
                "machine job is admitted; agent log:\n{}",
                agent_log.join("\n")
            );
        }
    };
    advance_to_done(slot, &store, &mut logger).await;

    let request_text = skarbiec_request
        .join()
        .expect("mock Skarbiec thread completes");
    let request_lower = request_text.to_ascii_lowercase();
    assert!(request_lower.starts_with("post /v1/items/read http/1.1"));
    assert!(request_lower.contains("x-consumer: workflow-test-agent"));
    assert!(request_lower.contains("authorization: bearer test-agent-grant"));
    assert!(request_text.contains(r#"{"id":"model-provider","field":"token"}"#));

    let completed = store
        .read_job("completed", &job_id)
        .await
        .unwrap()
        .expect("machine job completes");
    assert_eq!(completed.state, "completed");

    let download = temp.path().join("machine-artifacts");
    let artifacts = stado(
        &storage,
        &[
            "machine",
            "artifacts",
            &job_id,
            "--output-dir",
            download.to_str().unwrap(),
        ],
    );
    let artifacts = machine_result(&artifacts);
    let paths = artifacts["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|artifact| artifact["relative_path"].as_str())
        .collect::<Vec<_>>();
    assert!(paths.contains(&"rows.txt"));
    assert!(paths.contains(&"secret-length.txt"));
    assert!(paths.contains(&"command_output.log"));
    let rows = std::fs::read_to_string(download.join("rows.txt")).unwrap();
    let command_log =
        std::fs::read_to_string(download.join("command_output.log")).unwrap_or_default();
    assert_eq!(rows.trim(), "2", "command output: {command_log}");
    assert_eq!(
        std::fs::read_to_string(download.join("secret-length.txt"))
            .unwrap()
            .trim()
            .parse::<usize>()
            .unwrap(),
        WORKLOAD_SECRET.len()
    );
    assert_tree_omits(&storage, WORKLOAD_SECRET.as_bytes());
    assert!(agent_log
        .iter()
        .any(|line| line.contains(&format!("Job {job_id} completed"))));
}

#[tokio::test]
async fn workload_process_cannot_inherit_control_plane_credentials() {
    const CREDENTIALS: &[(&str, &str)] = &[
        ("STADO_API_TOKEN", "private-stado-api-token"),
        ("AWS_SECRET_ACCESS_KEY", "private-aws-secret"),
        ("AZURE_CLIENT_SECRET", "private-azure-secret"),
        (
            "GOOGLE_APPLICATION_CREDENTIALS",
            "/private/control-plane-service-account.json",
        ),
    ];
    let _environment = EnvGuard::set(CREDENTIALS);
    let temp = tempfile::tempdir().unwrap();
    let storage = temp.path().join("storage");
    let store = local_store(&storage);
    let command = "mkdir -p output; if [ -z \"${STADO_API_TOKEN+x}${AWS_SECRET_ACCESS_KEY+x}${AZURE_CLIENT_SECRET+x}${GOOGLE_APPLICATION_CREDENTIALS+x}\" ]; then printf clean > output/privacy.txt; else printf leaked > output/privacy.txt; fi";
    let job = Job::new("feedface", command);
    let _workdir = WorkdirGuard::new(&job.job_id);
    store.write_job("queue", &job).await.unwrap();

    let mut agent_log = Vec::new();
    let mut logger = |line: &str| agent_log.push(line.to_string());
    let slot = slots::start_slot(&store, job, "privacy-worker", &mut logger, "local", None)
        .await
        .unwrap()
        .expect("privacy workload is admitted");
    advance_to_done(slot, &store, &mut logger).await;

    assert_eq!(
        store
            .download_text("status/feedface/output/privacy.txt")
            .await
            .unwrap()
            .as_deref(),
        Some("clean")
    );
    for (_, secret) in CREDENTIALS {
        assert_tree_omits(&storage, secret.as_bytes());
    }
}

/// Recovery journey: an already-constructed storage facade survives a
/// temporary endpoint outage and reads the unchanged canonical payload after
/// the endpoint returns.
#[tokio::test]
async fn storage_client_recovers_after_temporary_endpoint_outage() {
    let temp = tempfile::tempdir().unwrap();
    let storage = temp.path().join("storage");
    let displaced = temp.path().join("storage-during-outage");
    let submitted = stado(&storage, &["submit", "echo survives-storage-outage"]);
    assert_success(&submitted, "pre-outage submit");
    let job_id = submitted_job_id(&submitted);
    let canonical_before = queue_blob(&storage, &job_id);
    let store = local_store(&storage);

    std::fs::rename(&storage, &displaced).unwrap();
    std::fs::write(&storage, b"storage endpoint unavailable").unwrap();
    let unavailable = store.read_job("queue", &job_id).await;
    assert!(
        unavailable.is_err(),
        "an unavailable endpoint must not be mistaken for a missing job"
    );

    std::fs::remove_file(&storage).unwrap();
    std::fs::rename(&displaced, &storage).unwrap();
    let recovered = store
        .read_job("queue", &job_id)
        .await
        .unwrap()
        .expect("job is readable after storage endpoint recovery");
    assert_eq!(recovered.job_id, job_id);
    assert_eq!(queue_blob(&storage, &job_id), canonical_before);
}

/// Recovery journey: one workload's failed verification reaches a canonical
/// terminal state, then the same agent/store path admits and completes the
/// next queued workload instead of wedging dispatch.
#[tokio::test]
async fn failed_workload_does_not_block_the_next_queued_job() {
    let temp = tempfile::tempdir().unwrap();
    let storage = temp.path().join("storage");
    let failed_submit = stado(
        &storage,
        &[
            "submit",
            "mkdir -p output && printf bad > output/result.txt",
            "--verify",
            "test -e output/never-created",
        ],
    );
    let recovered_submit = stado(
        &storage,
        &[
            "submit",
            "mkdir -p output && printf recovered > output/result.txt",
            "--verify",
            "test -s output/result.txt",
        ],
    );
    assert_success(&failed_submit, "failing workload submit");
    assert_success(&recovered_submit, "recovery workload submit");
    let failed_id = submitted_job_id(&failed_submit);
    let recovered_id = submitted_job_id(&recovered_submit);
    let _failed_workdir = WorkdirGuard::new(&failed_id);
    let _recovered_workdir = WorkdirGuard::new(&recovered_id);
    let store = local_store(&storage);
    let mut agent_log = Vec::new();
    let mut logger = |line: &str| agent_log.push(line.to_string());

    let failed_job = store
        .read_job("queue", &failed_id)
        .await
        .unwrap()
        .expect("failing workload is queued");
    let failed_slot = slots::start_slot(&store, failed_job, "recovery-agent", &mut logger, "local", None)
        .await
        .unwrap()
        .expect("failing workload is admitted");
    advance_to_done(failed_slot, &store, &mut logger).await;
    let failed = store
        .read_job("failed", &failed_id)
        .await
        .unwrap()
        .expect("verification failure reaches terminal failed state");
    assert!(
        failed
            .error
            .as_deref()
            .is_some_and(|error| error.contains("verification command failed")),
        "unexpected terminal failure: {:?}",
        failed.error
    );

    let recovered_job = store
        .read_job("queue", &recovered_id)
        .await
        .unwrap()
        .expect("next workload remains queued");
    let recovered_slot = slots::start_slot(
        &store,
        recovered_job,
        "recovery-agent",
        &mut logger,
        "local",
        None,
    )
    .await
    .unwrap()
    .expect("next workload is admitted after predecessor failure");
    advance_to_done(recovered_slot, &store, &mut logger).await;
    let completed = store
        .read_job("completed", &recovered_id)
        .await
        .unwrap()
        .expect("next workload completes");
    assert_eq!(completed.state, "completed");
    assert!(store
        .read_job("queue", &recovered_id)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
#[ignore = "requires STADO_LIVE_GCS_BUCKET and scoped stado-gcp credentials"]
async fn live_gcs_sandbox_round_trip_exercises_cas_listing_metadata_and_delete() {
    let bucket =
        std::env::var("STADO_LIVE_GCS_BUCKET").expect("STADO_LIVE_GCS_BUCKET must name sandbox");
    let backend = GcsBackend::new(&bucket).await.unwrap();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let prefix = format!("live-tests/stado-rs/{}/", std::process::id());
    let path = format!("{prefix}{nonce}.txt");
    let body = format!("stado-rs-live-gcs-{nonce}");

    assert!(backend.upload_text_if_absent(&path, &body).await.unwrap());
    assert!(!backend
        .upload_text_if_absent(&path, "collision")
        .await
        .unwrap());
    assert_eq!(
        backend.download_text(&path).await.unwrap().as_deref(),
        Some(body.as_str())
    );
    backend
        .set_metadata(
            &path,
            &std::collections::BTreeMap::from([("stado-live-test".into(), "true".into())]),
        )
        .await
        .unwrap();
    let listed = backend.list_blobs_with_meta(&prefix).await.unwrap();
    let object = listed
        .iter()
        .find(|object| object.name == path)
        .expect("live object appears in prefix listing");
    assert_eq!(
        object.metadata.get("stado-live-test").map(String::as_str),
        Some("true")
    );
    backend.delete(&path).await.unwrap();
    assert!(backend.download_text(&path).await.unwrap().is_none());
}

#[tokio::test]
#[ignore = "requires STADO_LIVE_S3_BUCKET, STADO_LIVE_S3_REGION, and scoped AWS identity"]
async fn live_s3_sandbox_round_trip_exercises_cas_listing_metadata_and_delete() {
    let bucket =
        std::env::var("STADO_LIVE_S3_BUCKET").expect("STADO_LIVE_S3_BUCKET must name sandbox");
    let region =
        std::env::var("STADO_LIVE_S3_REGION").expect("STADO_LIVE_S3_REGION must be configured");
    let backend = S3Backend::new(&bucket, &region).await.unwrap();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let prefix = format!("live-tests/stado-rs/{}/", std::process::id());
    let path = format!("{prefix}{nonce}.txt");
    let body = format!("stado-rs-live-s3-{nonce}");

    assert!(backend.upload_text_if_absent(&path, &body).await.unwrap());
    assert!(!backend
        .upload_text_if_absent(&path, "collision")
        .await
        .unwrap());
    assert_eq!(
        backend.download_text(&path).await.unwrap().as_deref(),
        Some(body.as_str())
    );
    backend
        .set_metadata(
            &path,
            &std::collections::BTreeMap::from([("stado-live-test".into(), "true".into())]),
        )
        .await
        .unwrap();
    let listed = backend.list_blobs_with_meta(&prefix).await.unwrap();
    assert!(listed.iter().any(|object| {
        object.name == path
            && object.metadata.get("stado-live-test").map(String::as_str) == Some("true")
    }));
    backend.delete(&path).await.unwrap();
    assert!(backend.download_text(&path).await.unwrap().is_none());
}

#[tokio::test]
#[ignore = "requires STADO_LIVE_AZURE_ACCOUNT, STADO_LIVE_AZURE_CONTAINER, and scoped Azure identity"]
async fn live_azure_sandbox_round_trip_exercises_cas_listing_metadata_and_delete() {
    let account = std::env::var("STADO_LIVE_AZURE_ACCOUNT")
        .expect("STADO_LIVE_AZURE_ACCOUNT must name sandbox");
    let container = std::env::var("STADO_LIVE_AZURE_CONTAINER")
        .expect("STADO_LIVE_AZURE_CONTAINER must name sandbox");
    let backend = AzureBlobBackend::new(&account, &container).unwrap();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let prefix = format!("live-tests/stado-rs/{}/", std::process::id());
    let path = format!("{prefix}{nonce}.txt");
    let body = format!("stado-rs-live-azure-{nonce}");

    assert!(backend.upload_text_if_absent(&path, &body).await.unwrap());
    assert!(!backend
        .upload_text_if_absent(&path, "collision")
        .await
        .unwrap());
    assert_eq!(
        backend.download_text(&path).await.unwrap().as_deref(),
        Some(body.as_str())
    );
    backend
        .set_metadata(
            &path,
            &std::collections::BTreeMap::from([("stado-live-test".into(), "true".into())]),
        )
        .await
        .unwrap();
    let listed = backend.list_blobs_with_meta(&prefix).await.unwrap();
    assert!(listed.iter().any(|object| {
        object.name == path
            && object.metadata.get("stado-live-test").map(String::as_str) == Some("true")
    }));
    backend.delete(&path).await.unwrap();
    assert!(backend.download_text(&path).await.unwrap().is_none());
}

#[tokio::test]
#[ignore = "creates and deletes a billable GCE sandbox VM; requires GCP_PROJECT, WC_BUCKET, and scoped GCP identity"]
async fn live_gcp_compute_creates_observes_and_deletes_sandbox_vm() {
    let project = std::env::var("GCP_PROJECT").expect("GCP_PROJECT must name sandbox");
    assert!(
        !project.trim().is_empty(),
        "GCP_PROJECT must name a non-empty sandbox"
    );
    let machine_type =
        std::env::var("STADO_LIVE_GCP_MACHINE_TYPE").unwrap_or_else(|_| "e2-micro".to_string());
    let image = std::env::var("STADO_LIVE_GCP_IMAGE")
        .unwrap_or_else(|_| "projects/debian-cloud/global/images/family/debian-12".to_string());
    let (image_project, image_name) = image
        .strip_prefix("projects/")
        .and_then(|value| value.split_once("/global/images/"))
        .unwrap_or_else(|| {
            panic!(
                "STADO_LIVE_GCP_IMAGE must use projects/<project>/global/images/<image>: {image}"
            )
        });
    let boot_disk_gb = "10".parse().expect("static boot disk size");
    let delete_timeout =
        std::time::Duration::from_secs("300".parse().expect("static GCE deletion timeout"));
    let poll_interval =
        std::time::Duration::from_secs("2".parse().expect("static GCE poll interval"));
    let name = format!(
        "stado-live-rs-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    );
    let provider = GcpProvider::from_env();
    let created = provider
        .create_instance(
            &name,
            &machine_type,
            "",
            boot_disk_gb,
            image_name,
            image_project,
            "#!/bin/sh\nshutdown -h now\n",
            false,
        )
        .await
        .unwrap()
        .expect("GCE sandbox VM was not created in any configured zone");

    let exists = provider.instance_exists(&created).await;
    let lifecycle = provider.instance_lifecycle_state(&created).await;
    let delete = provider.delete_instance(&created).await;
    assert!(
        delete.is_ok(),
        "failed to delete live GCE sandbox VM {created}: {delete:?}"
    );

    let deadline = std::time::Instant::now() + delete_timeout;
    let deleted = loop {
        match provider.instance_lifecycle_state(&created).await {
            Ok(None) => break true,
            Ok(Some(_)) if std::time::Instant::now() < deadline => {
                tokio::time::sleep(poll_interval).await;
            }
            Ok(Some(state)) => {
                panic!("GCE sandbox VM {created} was not deleted before timeout; state={state}")
            }
            Err(error) => panic!("failed while confirming deletion of {created}: {error}"),
        }
    };

    assert!(exists.unwrap(), "created VM was not observable");
    assert!(
        lifecycle.unwrap().is_some(),
        "created VM had no lifecycle state"
    );
    assert!(deleted);
}
