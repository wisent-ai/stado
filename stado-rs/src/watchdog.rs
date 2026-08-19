//! Box diagnostics watchdog — port of `stado/deploy/watchdog/cli.py`.
//!
//! Collects fault-isolated box diagnostics (systemctl status, journalctl
//! tails, ps, nvidia-smi, df, free, and a `gcloud storage ls capacity/`
//! probe), then uploads the JSON payload to
//! `gs://<bucket>/box_diagnostics/<host>.json` (plus
//! `box_diagnostics/<host>/latest.json`) every interval (default 60 s).
//!
//! Deviation: Python's `_upload` drives the google-cloud-storage SDK
//! directly; here the upload goes through [`JobStorage`] (per the port
//! plan), so the `WC_STORAGE_BACKEND=local` backend works too.
//!
//! The CLI uses argparse semantics (NOT click like the rest of the
//! package): usage/error text, exit code 2 on argument errors, and
//! `-h/--help` on stdout with exit 0. One argparse behavior is not
//! reproduced: Python's `int()` accepts arbitrarily large values and
//! unicode digits; here `--interval-s` parses as `i64`.

use std::path::Path;
use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::models::{isoformat_utc, json_dumps_pretty_sorted, py_str_repr};
use crate::procutil::{run_capture, Capture};
use crate::queue::{JobStorage, StorageError};

/// Python `DEFAULT_BUCKET` (the watchdog's own default, NOT config BUCKET).
pub const DEFAULT_BUCKET: &str = "wisent-compute";
/// Python `DEFAULT_INTERVAL_S`.
pub const DEFAULT_INTERVAL_S: i64 = 60;
/// Python `OUT_PREFIX`.
pub const OUT_PREFIX: &str = "box_diagnostics";
/// Local fallback path when the upload fails (Python `_write_local`).
pub const LOCAL_FALLBACK_PATH: &str = "/tmp/wisent_box_diagnostics_latest.json";

const STDOUT_TAIL_CHARS: usize = 12000;

/// Outcome of one diagnostic command (Python `subprocess.run` result or a
/// `TimeoutExpired` after the child was killed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
    Completed {
        rc: i32,
        stdout: String,
        stderr: String,
    },
    TimedOut {
        stdout: String,
        stderr: String,
    },
}

/// Fault-isolation seam: how a diagnostic command is executed. Tests inject
/// a fake; production uses [`SystemRunner`].
pub trait CommandRunner: Send + Sync {
    /// Run `argv` capturing output with a `timeout_s` kill deadline.
    /// `Err` = the process could not be spawned at all (Python's generic
    /// `except Exception` branch, e.g. `FileNotFoundError` when the binary
    /// is not installed on the box).
    fn run(&self, argv: &[String], timeout_s: u64) -> std::io::Result<RunOutcome>;
}

/// Production runner: real subprocesses via [`run_capture`].
pub struct SystemRunner;

impl CommandRunner for SystemRunner {
    fn run(&self, argv: &[String], timeout_s: u64) -> std::io::Result<RunOutcome> {
        Ok(match run_capture(argv, Duration::from_secs(timeout_s))? {
            Capture::Completed { rc, stdout, stderr } => {
                RunOutcome::Completed { rc, stdout, stderr }
            }
            Capture::TimedOut { stdout, stderr } => RunOutcome::TimedOut { stdout, stderr },
        })
    }
}

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

/// Python `_write_local`: local fallback copy on upload failure.
fn write_local(payload: &Value) {
    let text = json_dumps_pretty_sorted(payload);
    if let Err(err) = std::fs::write(LOCAL_FALLBACK_PATH, text) {
        tracing::warn!("could not write {LOCAL_FALLBACK_PATH}: {err}");
    }
}

/// Upload the payload through an explicit store (test seam).
pub async fn upload_with(store: &JobStorage, payload: &Value) -> Result<(), StorageError> {
    let host = payload
        .get("host")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let text = json_dumps_pretty_sorted(payload);
    store
        .upload_text(&format!("{OUT_PREFIX}/{host}.json"), &text)
        .await?;
    store
        .upload_text(&format!("{OUT_PREFIX}/{host}/latest.json"), &text)
        .await?;
    Ok(())
}

/// Python `_upload`: construct the storage handle for `bucket` and upload.
/// Construction failures count as upload failures (the Python code builds
/// the storage client inside `_upload` too).
async fn upload(bucket: &str, payload: &Value) -> Result<(), StorageError> {
    let store = JobStorage::with_bucket(bucket).await?;
    upload_with(&store, payload).await
}

/// Python `once`: collect -> upload -> print. Returns the process exit code
/// for this pass (0 uploaded, 1 upload failed and the local fallback was
/// written). `store` is the test seam; `None` constructs it from `bucket`.
pub async fn once_with(
    bucket: &str,
    runner: &dyn CommandRunner,
    store: Option<&JobStorage>,
) -> i32 {
    let mut payload = collect(bucket, runner);
    let reported_at = payload
        .get("reported_at")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let host = payload
        .get("host")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let result = match store {
        Some(store) => upload_with(store, &payload).await,
        None => upload(bucket, &payload).await,
    };
    match result {
        Ok(()) => {
            println!("{reported_at} uploaded {OUT_PREFIX}/{host}.json");
            0
        }
        Err(err) => {
            payload["upload_error"] = Value::from(err.to_string());
            write_local(&payload);
            println!("{reported_at} upload failed; wrote {LOCAL_FALLBACK_PATH}");
            1
        }
    }
}

/// Production single pass (system subprocesses, storage from config).
pub async fn once(bucket: &str) -> i32 {
    once_with(bucket, &SystemRunner, None).await
}

// ---------------------------------------------------------------------------
// argparse-compatible CLI
// ---------------------------------------------------------------------------

/// Parsed watchdog arguments (Python `argparse.Namespace` equivalent).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedArgs {
    pub bucket: String,
    pub interval_s: i64,
    pub once: bool,
}

/// Non-Ok parse results, mapped to argparse's exit behavior.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseOutcome {
    /// `-h`/`--help`: print [`help_text`] on stdout, exit 0.
    Help,
    /// Usage error: print usage + `{prog}: error: {msg}` on stderr, exit 2.
    Error(String),
}

/// argparse's two-line usage string for this parser (it wraps the option
/// list to 80 columns; the break lands before `[--once]`, continuation
/// aligned past `usage: {prog} `).
pub fn usage_text(prog: &str) -> String {
    let indent = " ".repeat("usage: ".len() + prog.len() + 1);
    format!("usage: {prog} [-h] [--bucket BUCKET] [--interval-s INTERVAL_S]\n{indent}[--once]")
}

/// argparse `--help` output (without the trailing newline `print` adds).
pub fn help_text(prog: &str) -> String {
    format!(
        "{}\n\nUpload workstation diagnostics to GCS.\n\noptions:\n  -h, --help            show this help message and exit\n  --bucket BUCKET\n  --interval-s INTERVAL_S\n  --once",
        usage_text(prog)
    )
}

const LONG_OPTIONS: [&str; 4] = ["--help", "--bucket", "--interval-s", "--once"];

/// Python `int(str)` for the `--interval-s` argument: surrounding
/// whitespace tolerated, optional sign, `_` digit separators allowed
/// between digits.
fn parse_python_int(raw: &str) -> Option<i64> {
    let trimmed = raw.trim();
    let digits = trimmed.strip_prefix(['+', '-']).unwrap_or(trimmed);
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
        || !digits.chars().all(|c| c.is_ascii_digit() || c == '_')
    {
        return None;
    }
    let cleaned: String = trimmed.chars().filter(|&c| c != '_').collect();
    cleaned.parse::<i64>().ok()
}

/// argparse-style long-option resolution with unambiguous prefix
/// abbreviation (`--buck` -> `--bucket`).
fn resolve_long(name: &str) -> Option<&'static str> {
    let matches: Vec<&&str> = LONG_OPTIONS
        .iter()
        .filter(|opt| opt.starts_with(name))
        .collect();
    match matches.as_slice() {
        [one] => Some(*one),
        _ => None,
    }
}

/// argparse `parse_args` for the watchdog parser. Byte-reproduces
/// argparse's error strings; exit behavior is described by
/// [`ParseOutcome`].
pub fn parse_args(_prog: &str, args: &[String]) -> Result<ParsedArgs, ParseOutcome> {
    let bucket_env = crate::capabilities::config_env(
        crate::capabilities::RuntimeFacet::Storage,
        crate::capabilities::StorageAdapter::Gcs.id(),
        "bucket",
    )
    .expect("GCS bucket binding is missing from the capability catalog");
    let default_bucket = std::env::var(bucket_env).unwrap_or_else(|_| DEFAULT_BUCKET.to_string());
    let mut parsed = ParsedArgs {
        bucket: default_bucket,
        interval_s: DEFAULT_INTERVAL_S,
        once: false,
    };
    let mut extras: Vec<String> = Vec::new();
    let mut positional_only = false;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        index += 1;
        if positional_only {
            extras.push(arg.clone());
            continue;
        }
        if arg == "--" {
            positional_only = true;
            continue;
        }
        if arg == "-h" {
            return Err(ParseOutcome::Help);
        }
        if let Some(long) = arg.strip_prefix("--") {
            if long.is_empty() {
                extras.push(arg.clone());
                continue;
            }
            let (name, inline_value) = match long.split_once('=') {
                Some((name, value)) => (name, Some(value.to_string())),
                None => (long, None),
            };
            let dashed = format!("--{name}");
            let Some(option) = resolve_long(&dashed) else {
                extras.push(arg.clone());
                continue;
            };
            match option {
                "--help" => return Err(ParseOutcome::Help),
                "--once" => parsed.once = true,
                "--bucket" | "--interval-s" => {
                    let value = match inline_value {
                        Some(value) => value,
                        None => {
                            let Some(next) = args.get(index) else {
                                return Err(ParseOutcome::Error(format!(
                                    "argument {option}: expected one argument"
                                )));
                            };
                            index += 1;
                            next.clone()
                        }
                    };
                    if option == "--bucket" {
                        parsed.bucket = value;
                    } else {
                        match parse_python_int(&value) {
                            Some(interval) => parsed.interval_s = interval,
                            None => {
                                return Err(ParseOutcome::Error(format!(
                                    "argument --interval-s: invalid int value: {}",
                                    py_str_repr(&value)
                                )));
                            }
                        }
                    }
                }
                _ => unreachable!("LONG_OPTIONS is exhaustive"),
            }
            continue;
        }
        extras.push(arg.clone());
    }
    if !extras.is_empty() {
        return Err(ParseOutcome::Error(format!(
            "unrecognized arguments: {}",
            extras.join(" ")
        )));
    }
    Ok(parsed)
}

/// The argparse CLI entry (used by `src/bin/stado_watchdog.rs`).
pub async fn cli_main() -> i32 {
    let argv: Vec<String> = std::env::args().collect();
    let prog = argv
        .first()
        .and_then(|arg0| Path::new(arg0).file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "stado-watchdog".to_string());
    let parsed = match parse_args(&prog, &argv[1..]) {
        Ok(parsed) => parsed,
        Err(ParseOutcome::Help) => {
            println!("{}", help_text(&prog));
            return 0;
        }
        Err(ParseOutcome::Error(message)) => {
            eprintln!("{}", usage_text(&prog));
            eprintln!("{prog}: error: {message}");
            return 2;
        }
    };
    if parsed.once {
        return once(&parsed.bucket).await;
    }
    loop {
        once(&parsed.bucket).await;
        let interval = parsed.interval_s.max(10) as u64;
        tokio::time::sleep(Duration::from_secs(interval)).await;
    }
}

