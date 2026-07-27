//! End-to-end `stado-fix` tests against the local storage backend:
//! failed/ blobs written as plain files, the golden fix prompt, per-job
//! state at failure_fixes/<jid>.json, and a stubbed `claude` CLI on PATH
//! for the --execute dispatch path.

use std::path::Path;
use std::process::{Command, Output};

const JOB_ID: &str = "deadbeef";
const COMMAND: &str = "python -m wisent.scripts.activations.raw.extract_and_upload --model foo";
const FAILED_AT: &str = "2026-05-22T01:02:03+00:00";

/// Golden: generated from the real Python module
/// (`stado.failure_fixer.format_fix_prompt`) on this record.
const EXPECTED_PROMPT: &str = "You are the wisent-compute autonomous failure-fixer.\nA wisent-compute job (job_id=deadbeef batch_id=batch-42) failed at 2026-05-22T01:02:03+00:00.\n\nDiagnose the root cause from the traceback below, ship the fix to the appropriate repo (wisent / wisent-tools / wisent-compute), publish the patched package to PyPI, then resubmit this exact command via `wc submit <command> --verify <verify_command>`. The fleet's local agents drift-pick-up the new PyPI release on their next loop; cloud agents self-terminate on drift so a fresh VM with the new version claims the resubmitted job.\n\nFailed command:\n  python -m wisent.scripts.activations.raw.extract_and_upload --model foo\n\nTraceback (last 4000 chars of stderr):\n---BEGIN TRACEBACK---\nTraceback line 1\nTraceback line 2\n---END TRACEBACK---\n\nConstraints: never introduce mocks, soft-defaults, or silent error absorption. Diagnose root cause and patch the cause; if the root is in an upstream dependency the wisent-compute team cannot patch, surface that clearly instead of inventing a workaround.";

fn write_failed_blob(storage: &Path) {
    let failed = storage.join("failed");
    std::fs::create_dir_all(&failed).unwrap();
    let body = serde_json::json!({
        "job_id": JOB_ID,
        "batch_id": "batch-42",
        "command": COMMAND,
        "error": "Traceback line 1\nTraceback line 2",
        "failed_at": FAILED_AT,
    });
    std::fs::write(
        failed.join(format!("{JOB_ID}.json")),
        serde_json::to_string(&body).unwrap(),
    )
    .unwrap();
}

fn write_state(storage: &Path, job_id: &str, attempts: i64) {
    let dir = storage.join("failure_fixes");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(format!("{job_id}.json")),
        serde_json::to_string(&serde_json::json!({"attempts": attempts})).unwrap(),
    )
    .unwrap();
}

fn fix(storage: &Path, args: &[&str], path: Option<&str>) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_stado-fix"));
    cmd.args(args)
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", storage)
        .env("STADO_CONFIG", storage.join("no-such-config.json"));
    if let Some(path) = path {
        cmd.env("PATH", path);
    }
    cmd.output().expect("stado-fix binary runs")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn scan_reports_undispatched_and_dispatched() {
    let dir = tempfile::tempdir().unwrap();
    let storage = dir.path();
    write_failed_blob(storage);

    let out = fix(storage, &["scan"], None);
    assert!(out.status.success(), "{}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(report["total_failures_scanned"], 1);
    assert_eq!(report["undispatched"], 1);
    assert_eq!(report["already_dispatched"], 0);
    assert_eq!(report["jobs"][0]["job_id"], JOB_ID);
    assert_eq!(report["jobs"][0]["batch_id"], "batch-42");
    assert_eq!(report["jobs"][0]["failed_at"], FAILED_AT);
    assert_eq!(report["jobs"][0]["attempts"], 0);
    assert_eq!(report["jobs"][0]["command_head"], COMMAND);

    // Prior state flips the job to already-dispatched.
    write_state(storage, JOB_ID, 1);
    let out = fix(storage, &["scan"], None);
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(report["undispatched"], 0);
    assert_eq!(report["already_dispatched"], 1);

    // --since filters the blob away entirely.
    let out = fix(storage, &["scan", "--since", "2026-06-01"], None);
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(report["total_failures_scanned"], 0);
}

#[test]
fn prompt_is_byte_exact_vs_python() {
    let dir = tempfile::tempdir().unwrap();
    let storage = dir.path();
    write_failed_blob(storage);
    let out = fix(storage, &["prompt", JOB_ID], None);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), format!("{EXPECTED_PROMPT}\n"));

    let out = fix(storage, &["prompt", "zzz00000"], None);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        stderr(&out),
        "no failed job 'zzz00000' in current failed/\n"
    );
}

#[test]
fn dispatch_dry_run_and_exhausted_cap() {
    let dir = tempfile::tempdir().unwrap();
    let storage = dir.path();
    write_failed_blob(storage);

    let out = fix(storage, &["dispatch", JOB_ID], None);
    assert!(out.status.success(), "{}", stderr(&out));
    let result: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(result["job_id"], JOB_ID);
    assert_eq!(result["status"], "dry_run");
    assert_eq!(result["attempts"], 0);
    assert_eq!(result["prompt_bytes"], 1042);
    assert_eq!(result["prompt_preview"].as_str().unwrap().len(), 500);
    // Dry run writes no state.
    assert!(!storage.join("failure_fixes/deadbeef.json").exists());

    // At FAILURE_FIXER_ATTEMPT_CAP the job is EXHAUSTED.
    write_state(storage, JOB_ID, 3);
    let out = fix(storage, &["dispatch", JOB_ID, "--execute"], None);
    let result: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(
        result,
        serde_json::json!({"job_id": JOB_ID, "status": "exhausted", "attempts": 3})
    );
}

#[test]
fn scan_dispatch_filters_and_skips_dispatched() {
    let dir = tempfile::tempdir().unwrap();
    let storage = dir.path();
    write_failed_blob(storage);

    // Pattern mismatch -> nothing dispatched.
    let out = fix(
        storage,
        &["scan-dispatch", "--command-pattern", "lm_eval"],
        None,
    );
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(report, serde_json::json!({"results": [], "count": 0}));

    // Pattern match -> one dry-run record.
    let out = fix(
        storage,
        &[
            "scan-dispatch",
            "--command-pattern",
            "raw.extract_and_upload",
        ],
        None,
    );
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(report["count"], 1);
    assert_eq!(report["results"][0]["status"], "dry_run");
    assert_eq!(report["results"][0]["job_id"], JOB_ID);

    // attempts>0 in state -> already_dispatched.
    write_state(storage, JOB_ID, 2);
    let out = fix(storage, &["scan-dispatch"], None);
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(
        report["results"][0],
        serde_json::json!({"job_id": JOB_ID, "status": "already_dispatched", "attempts": 2})
    );
}

#[test]
fn execute_dispatches_via_local_claude_cli() {
    let dir = tempfile::tempdir().unwrap();
    let storage = dir.path();
    write_failed_blob(storage);

    // Stub `claude` on PATH: record the argv, print canned output.
    let bin_dir = dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let claude = bin_dir.join("claude");
    std::fs::write(
        &claude,
        "#!/bin/sh\nprintf '%s\\n' \"$1\" > \"$CLAUDE_STUB_DIR/argv.txt\"\nprintf 'fixed it\\n'\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let path = format!("{}:/usr/bin:/bin", bin_dir.display());

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_stado-fix"));
    let out = cmd
        .args(["dispatch", JOB_ID, "--execute"])
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", storage)
        .env("STADO_CONFIG", storage.join("no-such-config.json"))
        .env("PATH", &path)
        .env("CLAUDE_STUB_DIR", dir.path())
        .output()
        .expect("stado-fix binary runs");
    assert!(out.status.success(), "{}", stderr(&out));
    let result: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(result["status"], "dispatched");
    assert_eq!(result["attempts"], 1);
    assert_eq!(result["returncode"], 0);
    assert_eq!(result["stdout_preview"], "fixed it\n");
    // The prompt was passed as `claude -p PROMPT`.
    assert_eq!(
        std::fs::read_to_string(dir.path().join("argv.txt")).unwrap(),
        "-p\n"
    );

    // Per-job state landed at failure_fixes/<jid>.json.
    let state: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(storage.join("failure_fixes/deadbeef.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(state["attempts"], 1);
    assert_eq!(state["last_returncode"], 0);
    assert_eq!(state["last_stdout_preview"], "fixed it\n");
    assert_eq!(state["batch_id"], "batch-42");
    assert_eq!(state["command"], COMMAND);
    assert_eq!(state["failed_at"], FAILED_AT);
    assert!(state["last_dispatched_at"].as_str().unwrap().ends_with('Z'));
}
