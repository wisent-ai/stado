//! The binary this journey drives, the home it owns, and the receipt kept for
//! every command it starts.
//!
//! Split out of `main.rs` so each file stays inside the three hundred line
//! limit this repository enforces on itself.

use std::path::Path;
use std::process::{Output, Stdio};
use std::time::Duration;

use serde_json::json;
use tokio::process::Command;
use tokio::time::timeout;

use crate::owned_home::{copy_ssh_identity, copy_stado_config};

/// How long one Stado command may take before this journey stops it. A real
/// Apple preparation reaches a remote host and waits on its GUI session, which
/// is why the budget is minutes rather than seconds.
const COMMAND_BUDGET: Duration = Duration::from_secs(360);

pub fn stado_binary() -> std::ffi::OsString {
    std::env::var_os("STADO_TEST_BINARY").unwrap_or_else(|| env!("CARGO_BIN_EXE_stado").into())
}

/// The home this journey owns, for every process it starts.
///
/// The journey reaches a registered Apple host, so it reads two things that
/// live in the operator's home: the ssh identity that host answers to, and the
/// Stado configuration naming the registry the host is registered in. Both are
/// copied in. What must not happen is the other direction: the product records
/// its last-known-good registry copy and its preparation state under `HOME`,
/// and a journey run with the operator's home writes all of that into the
/// operator's own `~/.stado`.
static HOME: std::sync::LazyLock<tempfile::TempDir> = std::sync::LazyLock::new(|| {
    let owned = tempfile::tempdir().expect("a home this journey owns");
    copy_ssh_identity(owned.path());
    copy_stado_config(owned.path());
    owned
});

pub fn home() -> &'static Path {
    HOME.path()
}

/// Run one Stado command to completion, retaining the command, its identity
/// and both output streams as Probierz artifacts before, during and after.
pub async fn run(args: &[&str]) -> Output {
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
        .env("HOME", home())
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
    let (status, timed_out) = match timeout(COMMAND_BUDGET, child.wait()).await {
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
