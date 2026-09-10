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

mod cases;
