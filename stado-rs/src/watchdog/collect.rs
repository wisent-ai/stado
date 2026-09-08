//! Diagnostics collection: the host/rounding/tail helpers, the byte-exact
//! diagnostic command set, one fault-isolated command result, the statvfs
//! disk report, and the assembled payload.

use std::path::Path;

use serde_json::{json, Map, Value};

use super::runner::{CommandRunner, RunOutcome};
use crate::models::{isoformat_utc, py_str_repr};

const STDOUT_TAIL_CHARS: usize = 12000;

/// `platform.node()` equivalent — `$HOSTNAME` first, then the `hostname`
/// binary (same helper shape as `queue::submit`).
pub(crate) fn hostname() -> String {
    if let Ok(name) = std::env::var("HOSTNAME") {
        if !name.is_empty() {
            return name;
        }
    }
    std::process::Command::new("hostname")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_default()
}

/// Python `round(x, 2)` — round-half-even on the binary value, which is
/// exactly what Rust's `{:.2}` float formatting does.
fn round2(x: f64) -> f64 {
    format!("{x:.2}")
        .parse()
        .expect("{:.2} of a finite f64 parses")
}

/// Python `round(x, 3)`.
fn round3(x: f64) -> f64 {
    format!("{x:.3}")
        .parse()
        .expect("{:.3} of a finite f64 parses")
}

/// Python `s[-n:]` on a `str` (character-based, not byte-based).
fn tail_chars(s: &str, n: usize) -> String {
    let count = s.chars().count();
    if count <= n {
        s.to_string()
    } else {
        s.chars().skip(count - n).collect()
    }
}

/// The diagnostic command set, byte-exact argvs from `cli.py::_collect`.
/// (name, argv, timeout_s)
fn commands(bucket: &str) -> Vec<(&'static str, Vec<String>, u64)> {
    let argv = |parts: &[&str]| parts.iter().map(|s| s.to_string()).collect();
    vec![
        (
            "systemctl-agent",
            argv(&[
                "systemctl",
                "status",
                "wisent-agent.service",
                "--no-pager",
                "-l",
            ]),
            12,
        ),
        (
            "systemctl-health",
            argv(&[
                "systemctl",
                "status",
                "wisent-host-health.timer",
                "--no-pager",
                "-l",
            ]),
            12,
        ),
        (
            "journal-agent",
            argv(&[
                "journalctl",
                "-u",
                "wisent-agent.service",
                "-n",
                "240",
                "--no-pager",
            ]),
            20,
        ),
        (
            "journal-health",
            argv(&[
                "journalctl",
                "-u",
                "wisent-host-health.service",
                "-n",
                "120",
                "--no-pager",
            ]),
            20,
        ),
        (
            "ps",
            argv(&[
                "ps",
                "-eo",
                "pid,ppid,stat,pcpu,pmem,comm,args",
                "--sort=-%cpu",
            ]),
            12,
        ),
        ("nvidia-smi", argv(&["nvidia-smi"]), 12),
        ("df", argv(&["df", "-h"]), 12),
        ("memory", argv(&["free", "-h"]), 12),
        (
            "capacity-list",
            vec![
                "gcloud".into(),
                "--quiet".into(),
                "storage".into(),
                "ls".into(),
                format!("gs://{bucket}/capacity/"),
            ],
            20,
        ),
    ]
}

/// Python `_run`: one fault-isolated command result as a JSON dict. Key
/// insertion order matches the Python dict literals (the upload is
/// `sort_keys=True`, so this only matters for readability).
fn run_one(name: &str, argv: &[String], timeout_s: u64, runner: &dyn CommandRunner) -> Value {
    let started = std::time::Instant::now();
    let elapsed = || round3(started.elapsed().as_secs_f64());
    let argv_json = Value::Array(argv.iter().map(|a| Value::from(a.as_str())).collect());
    match runner.run(argv, timeout_s) {
        Ok(RunOutcome::Completed { rc, stdout, stderr }) => json!({
            "name": name,
            "cmd": argv_json,
            "rc": rc,
            "elapsed_s": elapsed(),
            "stdout_tail": tail_chars(&stdout, STDOUT_TAIL_CHARS),
            "stderr_tail": tail_chars(&stderr, STDOUT_TAIL_CHARS),
        }),
        Ok(RunOutcome::TimedOut { stdout, stderr }) => json!({
            "name": name,
            "cmd": argv_json,
            "rc": Value::Null,
            "elapsed_s": elapsed(),
            "timeout_s": timeout_s,
            "stdout_tail": tail_chars(&stdout, STDOUT_TAIL_CHARS),
            "stderr_tail": tail_chars(&stderr, STDOUT_TAIL_CHARS),
            "timed_out": true,
        }),
        Err(err) => {
            // Python: f"{type(exc).__name__}: {exc}" — for the common case
            // (binary missing on the box) reproduce the FileNotFoundError
            // text; anything else degrades to a generic OSError label.
            let message = if err.kind() == std::io::ErrorKind::NotFound {
                format!(
                    "FileNotFoundError: [Errno 2] No such file or directory: {}",
                    py_str_repr(&argv[0])
                )
            } else {
                format!("OSError: {err}")
            };
            json!({
                "name": name,
                "cmd": argv_json,
                "rc": Value::Null,
                "elapsed_s": elapsed(),
                "error": message,
            })
        }
    }
}

/// Python `_disk`: `shutil.disk_usage(path)` in GiB. statvfs semantics:
/// total = f_blocks * f_frsize, used = (f_blocks - f_bfree) * f_frsize,
/// free = f_bavail * f_frsize.
fn disk(path: &str) -> Value {
    match nix::sys::statvfs::statvfs(Path::new(path)) {
        Ok(st) => {
            const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
            let frsize = st.fragment_size() as f64;
            let total = st.blocks() as f64 * frsize;
            let used = (st.blocks() - st.blocks_free()) as f64 * frsize;
            let free = st.blocks_available() as f64 * frsize;
            let used_pct = if st.blocks() > 0 {
                round2(used / total * 100.0)
            } else {
                0.0
            };
            json!({
                "path": path,
                "total_gb": round2(total / GIB),
                "used_gb": round2(used / GIB),
                "free_gb": round2(free / GIB),
                "used_pct": used_pct,
            })
        }
        Err(err) => {
            let message = if err == nix::errno::Errno::ENOENT {
                format!(
                    "FileNotFoundError: [Errno 2] No such file or directory: {}",
                    py_str_repr(path)
                )
            } else {
                format!("OSError: {err}")
            };
            json!({"path": path, "error": message})
        }
    }
}

/// Python `_collect`: assemble the full diagnostics payload.
pub fn collect(bucket: &str, runner: &dyn CommandRunner) -> Value {
    let host = hostname();
    let home = std::env::var("HOME").unwrap_or_default();
    let mut command_results = Map::new();
    for (name, argv, timeout_s) in commands(bucket) {
        command_results.insert(name.to_string(), run_one(name, &argv, timeout_s, runner));
    }
    json!({
        "schema": "wisent-box-diagnostics-v1",
        "reported_at": isoformat_utc(chrono::Utc::now()),
        "host": host,
        "bucket": bucket,
        "pid": std::process::id(),
        "disk": [disk("/"), disk(&home), disk("/tmp")],
        "commands": Value::Object(command_results),
    })
}
