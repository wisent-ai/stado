//! Real, prompt-free preparation on the explicitly registered Apple host. No
//! Apple authentication, notification, browser, or CuaDriver launch occurs.
//! The workload capability retired `host gui-automation …`; until this
//! revision both stories called those verbs and died before reaching a host.

use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::{Output, Stdio};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::time::timeout;

/// Apple-only preparation: the signed helper and its Accessibility grant.
const APPLE_ONLY_PLAN: &str = r#"{"schema":"wisent.gui-automation-plan.v1",
"operation":"grant-accessibility","apple_only":true}"#;
/// Contract, not tuning: the API refuses a request above its own bound, and a
/// host command outrunning its bound fails with its output retained.
const API_COMMAND_SECONDS: u32 = 300;
const COMMAND_SECONDS: u64 = 360;

#[derive(Debug, Deserialize)]
struct Report {
    target: String,
    ssh_target: String,
    items: Vec<(String, String)>,
    error: Option<String>,
}

impl Report {
    fn state(&self) -> BTreeMap<&str, &str> {
        self.items
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect()
    }

    /// Everything this preparation must leave exactly as it found it.
    fn unrelated_state(&self) -> BTreeMap<&str, &str> {
        let mut state = self.state();
        state.retain(|key, _| !key.starts_with("apple-challenge-"));
        state
    }

    fn assert_ready(&self, target: &str) {
        assert_eq!(self.target, target);
        assert_eq!(self.error, None, "{self:#?}");
        let state = self.state();
        assert_eq!(state.get("apple-challenge-helper-version"), Some(&"2"));
        assert_eq!(state.get("apple-challenge-accessibility"), Some(&"granted"));
        assert_eq!(
            state.get("apple-challenge-ready"),
            Some(&"yes"),
            "{self:#?}"
        );
        assert_eq!(state["console"], state["accessibility-user"]);
    }
}

fn stado_binary() -> std::ffi::OsString {
    std::env::var_os("STADO_TEST_BINARY").unwrap_or_else(|| env!("CARGO_BIN_EXE_stado").into())
}

fn artifacts() -> PathBuf {
    let named = std::env::var_os("PROBIERZ_ARTIFACTS");
    PathBuf::from(named.expect("Probierz artifact directory is required"))
}

fn retain(name: &str, value: &Value) {
    let bytes = serde_json::to_vec_pretty(value).expect("the retained result is JSON");
    std::fs::write(artifacts().join(name), bytes).expect("retain the recorded result");
}

fn plan_file() -> PathBuf {
    let path = artifacts().join("apple-only-plan.json");
    std::fs::write(&path, APPLE_ONLY_PLAN).expect("retain the submitted plan");
    path
}

/// One real Stado command; argv, exit status and complete output are retained.
async fn run(args: &[&str]) -> Output {
    let binary = stado_binary();
    let stem = format!("apple-command-{}", uuid::Uuid::new_v4());
    let stdout_path = artifacts().join(format!("{stem}.stdout.log"));
    let stderr_path = artifacts().join(format!("{stem}.stderr.log"));
    let started = chrono::Utc::now().to_rfc3339();
    let mut child = Command::new(&binary)
        .args(args)
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(std::fs::File::create(&stdout_path).expect("retain command stdout"))
        .stderr(std::fs::File::create(&stderr_path).expect("retain command stderr"))
        .kill_on_drop(true)
        .spawn()
        .expect("Stado binary starts");
    let pid = child.id();
    let (status, timed_out) =
        match timeout(Duration::from_secs(COMMAND_SECONDS), child.wait()).await {
            Ok(status) => (status.expect("reap Stado"), false),
            Err(_) => {
                child.kill().await.expect("stop the timed-out command");
                (child.wait().await.expect("reap timed-out Stado"), true)
            }
        };
    retain(
        &format!("{stem}.json"),
        &json!({
            "binary": binary.to_string_lossy(), "args": args, "pid": pid,
            "status": if timed_out { "timed-out" } else { "exited" },
            "exit_code": status.code(), "started_at": started,
            "completed_at": chrono::Utc::now().to_rfc3339(),
            "stdout": stdout_path, "stderr": stderr_path,
        }),
    );
    let output = Output {
        status,
        stdout: std::fs::read(stdout_path).expect("read retained stdout"),
        stderr: std::fs::read(stderr_path).expect("read retained stderr"),
    };
    eprintln!("COMMAND {args:?} EXIT {:?}", output.status.code());
    assert!(!timed_out, "the Stado operation outran its bound");
    output
}

fn report(output: &Output) -> Report {
    let said = String::from_utf8_lossy(&output.stderr).to_string();
    let code = output.status.code();
    assert!(output.status.success(), "Stado exited {code:?}: {said}");
    serde_json::from_slice(&output.stdout).expect("Stado returned its complete JSON report")
}

async fn status(target: &str) -> Report {
    report(
        &run(&[
            "workload",
            "status",
            "gui-automation",
            "--target",
            target,
            "--json",
        ])
        .await,
    )
}

async fn prepare(target: &str, plan: &str) -> Output {
    let verbs = ["workload", "run", "gui-automation"];
    run(&[&verbs[..], &["--target", target, "--plan", plan, "--json"]].concat()).await
}

fn registered_host() -> String {
    let machine = (std::env::consts::OS, std::env::consts::ARCH);
    assert_eq!(machine, ("macos", "aarch64"), "this needs the real Mac");
    let target = std::env::var("STADO_APPLE_PREPARATION_HOST")
        .expect("STADO_APPLE_PREPARATION_HOST must name the registered Apple host");
    assert!(!target.trim().is_empty(), "the Apple host must be explicit");
    target
}

#[tokio::test]
#[ignore = "Probierz supplies the explicit registered Darwin ARM64 Apple preparation host"]
async fn apple_readiness_observes_the_registered_host_without_preparing_it() {
    let target = registered_host();
    let observed = status(&target).await;
    observed.assert_ready(&target);
    verify_native_api(&target, &observed, false).await;
}

#[tokio::test]
#[ignore = "Probierz supplies the explicit registered Darwin ARM64 Apple preparation host"]
async fn apple_only_preparation_preserves_other_gui_state_and_works_through_the_desktop_api() {
    let target = registered_host();
    let file = plan_file();
    let plan = file.to_str().expect("the plan path is UTF-8");
    let unknown = format!("probierz-apple-unknown-{}", uuid::Uuid::new_v4());
    let refusal = prepare(&unknown, plan).await;
    assert_eq!(refusal.status.code(), Some(1));
    let said = String::from_utf8_lossy(&refusal.stderr).to_string();
    let sentence = format!("target '{unknown}' is not declared; add it to the canonical registry");
    assert!(said.contains(&sentence), "{said}");
    let before = status(&target).await;
    let prepared = report(&prepare(&target, plan).await);
    assert_eq!(prepared.error, None, "{prepared:#?}");
    let after_cli = status(&target).await;
    after_cli.assert_ready(&target);
    assert_eq!(after_cli.ssh_target, before.ssh_target);
    assert_eq!(
        after_cli.unrelated_state(),
        before.unrelated_state(),
        "Apple-only preparation changed unrelated GUI state"
    );
    verify_native_api(&target, &after_cli, true).await;
}

async fn verify_native_api(target: &str, after_cli: &Report, prepare: bool) {
    let work =
        PathBuf::from(std::env::var_os("HOME").expect("HOME is required")).join(".stado/work");
    std::fs::create_dir_all(&work).expect("create test work root");
    let isolated = tempfile::Builder::new()
        .prefix("apple-preparation-api-")
        .tempdir_in(work)
        .expect("create isolated API store");
    // A real second instance of this exact product binary, isolated store.
    let mut server = Command::new(stado_binary())
        .args(["dashboard", "--bind", "127.0.0.1", "--port", "0"])
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", isolated.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("start the exact Stado API binary");
    let mut lines = BufReader::new(server.stderr.take().expect("capture API stderr")).lines();
    let endpoint = timeout(Duration::from_secs(60), async {
        while let Some(line) = lines.next_line().await.expect("read API startup") {
            eprintln!("API {line}");
            if let Some(endpoint) = line.strip_prefix("[dashboard] listening on ") {
                return endpoint.to_string();
            }
        }
        panic!("Stado API exited before listening");
    })
    .await
    .expect("the Stado API must bind within its startup bound");
    let logs = tokio::spawn(async move {
        while let Ok(Some(line)) = lines.next_line().await {
            eprintln!("API {line}");
        }
    });
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(330))
        .build()
        .expect("build the API client");
    let health = http.get(format!("{endpoint}/healthz")).send().await;
    assert_eq!(health.expect("read API health").status(), StatusCode::OK);
    let read = json!({
        "args": ["workload", "status", "gui-automation", "--target", target, "--json"],
        "timeout_seconds": API_COMMAND_SECONDS
    });
    let (code, body) = call(&http, &endpoint, &read, "apple-api-readiness.json").await;
    assert_eq!(code, StatusCode::OK, "{body}");
    let result: Value = serde_json::from_str(&body).expect("decode API readiness result");
    assert_eq!(result["exit_code"], 0, "{result}");
    let observed: Report =
        serde_json::from_str(result["stdout"].as_str().expect("capture readiness stdout"))
            .expect("decode the actual host readiness receipt");
    observed.assert_ready(target);
    assert_eq!(
        observed.state(),
        after_cli.state(),
        "the read-only Desktop API changed observed host state"
    );
    // The plan travels as staged input, the way the native client sends it.
    let unknown = format!("probierz-apple-unconfirmed-{}", uuid::Uuid::new_v4());
    let arguments = |host: &str| {
        json!([
            "workload",
            "run",
            "gui-automation",
            "--target",
            host,
            "--plan",
            "$INPUT",
            "--json"
        ])
    };
    let unconfirmed = json!({"args": arguments(&unknown), "input": APPLE_ONLY_PLAN});
    let (code, body) = call(&http, &endpoint, &unconfirmed, "apple-api-refusal.json").await;
    assert_eq!(code, StatusCode::FORBIDDEN, "{body}");
    let refused: Value = serde_json::from_str(&body).expect("read the actual API refusal");
    let sentence = "mutating commands require explicit RUN_MUTATION confirmation";
    assert_eq!(refused["error"], sentence);
    if prepare {
        let request = json!({
            "args": arguments(target), "input": APPLE_ONLY_PLAN,
            "confirmation": "RUN_MUTATION", "timeout_seconds": API_COMMAND_SECONDS
        });
        let (code, body) = call(&http, &endpoint, &request, "apple-api-preparation.json").await;
        assert_eq!(code, StatusCode::OK, "{body}");
        let result: Value = serde_json::from_str(&body).expect("decode API result");
        assert_eq!(result["exit_code"], 0, "{result}");
        assert_eq!(result["ok"], true, "{result}");
        let reused: Report =
            serde_json::from_str(result["stdout"].as_str().expect("capture command stdout"))
                .expect("decode the actual preparation receipt");
        assert_eq!(
            reused.state().get("apple-challenge-helper"),
            Some(&"reused")
        );
        let after_api = status(target).await;
        after_api.assert_ready(target);
        assert_eq!(
            after_api.state(),
            after_cli.state(),
            "repeating preparation through the API changed observed host state"
        );
        assert_eq!(after_api.ssh_target, after_cli.ssh_target);
    }
    server
        .kill()
        .await
        .expect("stop only the isolated test API");
    server.wait().await.expect("reap the isolated API process");
    logs.await.expect("retain the remaining API log");
}
/// One operator-API request, with its status and complete body retained.
async fn call(
    http: &reqwest::Client,
    endpoint: &str,
    request: &Value,
    name: &str,
) -> (StatusCode, String) {
    let response = http
        .post(format!("{endpoint}/api/operator/run"))
        .header("X-Stado-Action", "operator-command")
        .json(request)
        .send()
        .await
        .expect("reach the native operator API");
    let status = response.status();
    let body = response.text().await.expect("retain the API response");
    retain(
        name,
        &json!({"request": request, "http_status": status.as_u16(), "body": body}),
    );
    (status, body)
}
