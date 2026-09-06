//! Real, prompt-free preparation on the explicitly registered Apple host.
//! No Apple authentication, notification, browser, or CuaDriver launch occurs.

use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::process::{Output, Stdio};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::time::timeout;

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
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect()
    }

    fn unrelated_state(&self) -> BTreeMap<&str, &str> {
        self.state()
            .into_iter()
            .filter(|(key, _)| !key.starts_with("apple-challenge-"))
            .collect()
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

async fn run(args: &[&str]) -> Output {
    let binary = stado_binary();
    let artifacts = std::path::PathBuf::from(
        std::env::var_os("PROBIERZ_ARTIFACTS").expect("Probierz artifact directory is required"),
    );
    let stem = format!("apple-command-{}", uuid::Uuid::new_v4());
    let stdout_path = artifacts.join(format!("{stem}.stdout.log"));
    let stderr_path = artifacts.join(format!("{stem}.stderr.log"));
    let receipt_path = artifacts.join(format!("{stem}.json"));
    let mut receipt = json!({
        "binary": binary.to_string_lossy(),
        "args": args,
        "status": "prepared",
        "recorded_at": chrono::Utc::now().to_rfc3339(),
        "stdout": stdout_path,
        "stderr": stderr_path,
    });
    std::fs::write(&receipt_path, serde_json::to_vec_pretty(&receipt).unwrap())
        .expect("retain the command before starting it");
    eprintln!(
        "COMMAND {binary:?} {args:?}\nRECEIPT {}",
        receipt_path.display()
    );
    let mut child = Command::new(binary)
        .args(args)
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(std::fs::File::create(&stdout_path).expect("retain command stdout"))
        .stderr(std::fs::File::create(&stderr_path).expect("retain command stderr"))
        .kill_on_drop(true)
        .spawn()
        .expect("Stado binary starts");
    receipt["status"] = json!("started");
    receipt["pid"] = json!(child.id());
    receipt["started_at"] = json!(chrono::Utc::now().to_rfc3339());
    std::fs::write(&receipt_path, serde_json::to_vec_pretty(&receipt).unwrap())
        .expect("retain the started process identity");
    let (status, timed_out) = match timeout(Duration::from_secs(360), child.wait()).await {
        Ok(status) => (status.expect("reap Stado"), false),
        Err(_) => {
            child
                .kill()
                .await
                .expect("stop the timed-out Stado command");
            (child.wait().await.expect("reap timed-out Stado"), true)
        }
    };
    receipt["status"] = json!(if timed_out { "timed-out" } else { "exited" });
    receipt["exit_code"] = json!(status.code());
    receipt["process_status"] = json!(status.to_string());
    receipt["completed_at"] = json!(chrono::Utc::now().to_rfc3339());
    std::fs::write(&receipt_path, serde_json::to_vec_pretty(&receipt).unwrap())
        .expect("retain the command result");
    let output = Output {
        status,
        stdout: std::fs::read(stdout_path).expect("read retained stdout"),
        stderr: std::fs::read(stderr_path).expect("read retained stderr"),
    };
    eprintln!(
        "EXIT {:?}\nSTDOUT\n{}\nSTDERR\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !timed_out,
        "Stado operation exceeded 360 seconds; its command and output are retained"
    );
    output
}

fn report(output: &Output) -> Report {
    assert!(
        output.status.success(),
        "Stado exited {:?}; full output was retained above",
        output.status.code()
    );
    serde_json::from_slice(&output.stdout).expect("Stado returned its complete JSON report")
}

async fn status(target: &str) -> Report {
    report(&run(&["host", "gui-automation", "status", target, "--json"]).await)
}

#[tokio::test]
#[ignore = "Probierz supplies the explicit registered Darwin ARM64 Apple preparation host"]
async fn apple_readiness_observes_the_registered_host_without_preparing_it() {
    assert_eq!(std::env::consts::OS, "macos");
    assert_eq!(std::env::consts::ARCH, "aarch64");
    let target = std::env::var("STADO_APPLE_PREPARATION_HOST")
        .expect("STADO_APPLE_PREPARATION_HOST must name the registered Apple host");
    assert!(!target.trim().is_empty(), "the Apple host must be explicit");
    let observed = status(&target).await;
    observed.assert_ready(&target);
    verify_native_api(&target, &observed, false).await;
}

#[tokio::test]
#[ignore = "Probierz supplies the explicit registered Darwin ARM64 Apple preparation host"]
async fn apple_only_preparation_preserves_other_gui_state_and_works_through_the_desktop_api() {
    assert_eq!(
        std::env::consts::OS,
        "macos",
        "this journey requires the real dedicated Mac"
    );
    assert_eq!(std::env::consts::ARCH, "aarch64");
    let target = std::env::var("STADO_APPLE_PREPARATION_HOST")
        .expect("STADO_APPLE_PREPARATION_HOST must name the registered Apple host");
    assert!(!target.trim().is_empty(), "the Apple host must be explicit");

    let unknown = format!("probierz-apple-unknown-{}", uuid::Uuid::new_v4());
    let refusal = run(&[
        "host",
        "gui-automation",
        "grant-accessibility",
        &unknown,
        "--apple-only",
        "--json",
    ])
    .await;
    assert_eq!(refusal.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&refusal.stderr)
        .contains(&format!("unknown registry target: {unknown}")));

    let before = status(&target).await;
    let prepared = report(
        &run(&[
            "host",
            "gui-automation",
            "grant-accessibility",
            &target,
            "--apple-only",
            "--json",
        ])
        .await,
    );
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
    // The API is a real second instance of this exact product binary. Its
    // local store is isolated; operator children retain the configured real
    // Stado host/credential clients, not this server's local-store override.
    let work = std::path::PathBuf::from(std::env::var_os("HOME").expect("HOME is required"))
        .join(".stado/work");
    std::fs::create_dir_all(&work).expect("create test work root");
    let isolated = tempfile::Builder::new()
        .prefix("apple-preparation-api-")
        .tempdir_in(work)
        .expect("create isolated API store");
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
    .expect("Stado API must bind within 60 seconds");
    let logs = tokio::spawn(async move {
        while let Ok(Some(line)) = lines.next_line().await {
            eprintln!("API {line}");
        }
    });
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(330))
        .build()
        .unwrap();
    let health = http
        .get(format!("{endpoint}/healthz"))
        .send()
        .await
        .expect("read actual API readiness");
    assert_eq!(health.status(), reqwest::StatusCode::OK);
    let response = http
        .post(format!("{endpoint}/api/operator/run"))
        .header("X-Stado-Action", "operator-command")
        .json(&json!({
            "args": ["host", "gui-automation", "status", target, "--json"],
            "timeout_seconds": 300
        }))
        .send()
        .await
        .expect("read real Apple readiness without mutation confirmation");
    let status_code = response.status();
    let body = response
        .text()
        .await
        .expect("retain API readiness response");
    eprintln!("API READINESS HTTP {status_code}\n{body}");
    let artifacts = std::path::PathBuf::from(
        std::env::var_os("PROBIERZ_ARTIFACTS").expect("Probierz artifact directory is required"),
    );
    std::fs::write(
        artifacts.join("apple-api-readiness.json"),
        serde_json::to_vec_pretty(&json!({
            "http_status": status_code.as_u16(),
            "body": body,
        }))
        .unwrap(),
    )
    .expect("retain the native API readiness response");
    assert_eq!(status_code, reqwest::StatusCode::OK);
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

    let args = [
        "host",
        "gui-automation",
        "grant-accessibility",
        target,
        "--apple-only",
        "--json",
    ];

    let unknown = format!("probierz-apple-unconfirmed-{}", uuid::Uuid::new_v4());
    let mut unconfirmed = args;
    unconfirmed[3] = &unknown;
    let refused = http
        .post(format!("{endpoint}/api/operator/run"))
        .header("X-Stado-Action", "operator-command")
        .json(&json!({"args": unconfirmed}))
        .send()
        .await
        .expect("send preparation without confirmation");
    assert_eq!(refused.status(), reqwest::StatusCode::FORBIDDEN);
    let refused: Value = refused.json().await.expect("read the actual API refusal");
    eprintln!("API REFUSAL {refused}");
    std::fs::write(
        artifacts.join("apple-api-refusal.json"),
        serde_json::to_vec_pretty(&refused).unwrap(),
    )
    .expect("retain the native API mutation refusal");
    assert_eq!(
        refused["error"],
        "mutating commands require explicit RUN_MUTATION confirmation"
    );
    if prepare {
        let response = http
            .post(format!("{endpoint}/api/operator/run"))
            .header("X-Stado-Action", "operator-command")
            .json(&json!({"args": args, "confirmation": "RUN_MUTATION", "timeout_seconds": 300}))
            .send()
            .await
            .expect("prepare the same real host through the Desktop API");
        let status_code = response.status();
        let body = response
            .text()
            .await
            .expect("retain API preparation response");
        eprintln!("API PREPARATION HTTP {status_code}\n{body}");
        assert_eq!(status_code, reqwest::StatusCode::OK);
        let result: Value = serde_json::from_str(&body).expect("decode API result");
        assert_eq!(result["exit_code"], 0, "{result}");
        assert_eq!(result["ok"], true, "{result}");
        let reused: Report =
            serde_json::from_str(result["stdout"].as_str().expect("capture command stdout"))
                .expect("decode the actual partial-or-complete preparation receipt");
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
