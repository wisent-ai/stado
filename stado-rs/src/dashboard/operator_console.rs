//! Bounded native Desktop access to the canonical CLI implementation.
//! Requests contain argv, never a shell command. The retired HTML catalog is
//! not needed by native clients and is intentionally not restored.

use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use super::{operator_auth, send_json, Request, Response};

const MAX_ARGUMENTS: usize = 96;
const MAX_ARGUMENT_BYTES: usize = 4096;
const MAX_INPUT_BYTES: usize = 1024 * 1024;
pub(super) const MAX_REQUEST_BYTES: usize = MAX_INPUT_BYTES + 128 * 1024;
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
// A fixed one-hour native log can exceed the ordinary command preview. Keep
// its JSON receipt intact for Desktop while retaining a bounded capture.
const MAX_RETAINED_LOG_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
const MAX_TIMEOUT_SECONDS: u64 = 300;
const MUTATION_CONFIRMATION: &str = "RUN_MUTATION";
const INPUT_PLACEHOLDER: &str = "$INPUT";
static INPUT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

const ALLOWED_FAMILIES: &[&str] = &[
    "alerts",
    "artifact",
    "azure",
    "billing",
    "blast-radius",
    "bootstrap",
    "cancel",
    "capabilities",
    "cloudflare",
    "config",
    "cost",
    "disk-cleanup",
    "doctor",
    "fleet",
    "host",
    "identity",
    "inference",
    "install-disk-cleanup",
    "instances",
    "job",
    "machine",
    "mail",
    "optimize",
    "overview",
    "placement",
    "profiles",
    "quota",
    "queue",
    "recovery",
    "registry",
    "release",
    "resources",
    "results",
    "schedule",
    "secrets",
    "service",
    "status",
    "storage",
    "submit",
    "vast",
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunRequest {
    args: Vec<String>,
    #[serde(default)]
    input: Option<String>,
    #[serde(default)]
    confirmation: String,
    #[serde(default = "default_timeout")]
    timeout_seconds: u64,
}

fn default_timeout() -> u64 {
    120
}

struct ConsoleError {
    status: u16,
    message: String,
}
impl ConsoleError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: 400,
            message: message.into(),
        }
    }
    fn forbidden(message: impl Into<String>) -> Self {
        Self {
            status: 403,
            message: message.into(),
        }
    }
    fn unavailable(message: impl Into<String>) -> Self {
        Self {
            status: 503,
            message: message.into(),
        }
    }
}

fn is_retained_log_request(args: &[String]) -> bool {
    args.first().is_some_and(|arg| arg == "host")
        && args.get(1).is_some_and(|arg| arg == "exec")
        && args
            .iter()
            .position(|arg| arg == "--")
            .is_some_and(|separator| {
                crate::deploy::host_exec::is_retained_log_read(&args[separator + 1..])
            })
}

fn is_read_only(args: &[String]) -> bool {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        return true;
    }
    let family = args.first().map(String::as_str).unwrap_or("");
    let operation = args.get(1).map(String::as_str).unwrap_or("");
    let detail = args.get(2).map(String::as_str).unwrap_or("");
    if family == "azure" && operation == "unusual-activity" {
        return detail == "diagnose";
    }
    if family == "host" && operation == "gui-automation" {
        return detail == "status";
    }
    if family == "host" && operation == "exec" {
        return is_retained_log_request(args);
    }
    if matches!(
        family,
        "capabilities"
            | "overview"
            | "profiles"
            | "status"
            | "doctor"
            | "results"
            | "cost"
            | "blast-radius"
    ) {
        return true;
    }
    matches!(
        (family, operation),
        (
            "artifact",
            "list" | "show" | "resolve" | "verify" | "lineage"
        ) | ("billing", "show")
            | (
                "fleet",
                "list" | "status" | "catalog" | "doctor" | "pending" | "methods"
            )
            | (
                "host",
                "health" | "inventory" | "uptime" | "ping" | "disk" | "vaults"
            )
            | ("identity", "list" | "verify")
            | (
                "inference",
                "list" | "status" | "logs" | "plan-logs" | "doctor" | "verify" | "blockers"
            )
            | ("instances", "list")
            | ("machine", "status" | "logs" | "artifacts")
            | ("optimize", "status" | "explain")
            | ("queue", "status")
            | ("quota", "show" | "catalog" | "requests" | "azure-replies")
            | (
                "registry",
                "validate" | "pull" | "self" | "doctor" | "beacon-age"
            )
            | ("release", "catalog" | "status")
            | ("resources", "show" | "verify" | "operations")
            | ("schedule", "list" | "show")
            | ("secrets", "ls" | "doctor" | "inspect-vault")
            | (
                "service",
                "directory"
                    | "list"
                    | "onboarding-catalog"
                    | "status"
                    | "show"
                    | "logs"
                    | "env"
                    | "auth-check"
            )
            | (
                "storage",
                "ls" | "stat" | "cat" | "verify" | "objects" | "url"
            )
            | ("vast", "status")
            | ("alerts", "channels")
    )
}

fn validate(request: &RunRequest) -> Result<(), ConsoleError> {
    if request.args.is_empty() || request.args.len() > MAX_ARGUMENTS {
        return Err(ConsoleError::bad_request(format!(
            "args must contain 1 to {MAX_ARGUMENTS} values"
        )));
    }
    if request
        .args
        .iter()
        .any(|arg| arg.is_empty() || arg.len() > MAX_ARGUMENT_BYTES || arg.contains('\0'))
    {
        return Err(ConsoleError::bad_request(
            "arguments must be non-empty, bounded strings without NUL bytes",
        ));
    }
    if !ALLOWED_FAMILIES.contains(&request.args[0].as_str()) {
        return Err(ConsoleError::forbidden(format!(
            "command family {:?} is not available in the Desktop API",
            request.args[0]
        )));
    }
    if request.timeout_seconds == 0 || request.timeout_seconds > MAX_TIMEOUT_SECONDS {
        return Err(ConsoleError::bad_request(format!(
            "timeout_seconds must be between 1 and {MAX_TIMEOUT_SECONDS}"
        )));
    }
    if request.input.as_ref().map_or(0, String::len) > MAX_INPUT_BYTES {
        return Err(ConsoleError::bad_request(format!(
            "input exceeds the {MAX_INPUT_BYTES}-byte limit"
        )));
    }
    if request.args.iter().any(|arg| arg == INPUT_PLACEHOLDER) && request.input.is_none() {
        return Err(ConsoleError::bad_request(
            "$INPUT requires content in the input editor",
        ));
    }
    if !is_read_only(&request.args) && request.confirmation != MUTATION_CONFIRMATION {
        return Err(ConsoleError::forbidden(
            "mutating commands require explicit RUN_MUTATION confirmation",
        ));
    }
    Ok(())
}

struct StagedInput(PathBuf);
impl Drop for StagedInput {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

async fn stage_input(content: &str) -> Result<StagedInput, ConsoleError> {
    let home = std::env::var_os("HOME")
        .ok_or_else(|| ConsoleError::unavailable("HOME is required to stage operator input"))?;
    let directory = PathBuf::from(home).join(".stado/work/operator-input");
    tokio::fs::create_dir_all(&directory)
        .await
        .map_err(|error| {
            ConsoleError::unavailable(format!(
                "could not create operator input directory: {error}"
            ))
        })?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = INPUT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let path = directory.join(format!("{}-{now}-{sequence}.json", std::process::id()));
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&path)
        .await
        .map_err(|error| ConsoleError::unavailable(format!("could not stage input: {error}")))?;
    let staged = StagedInput(path);
    file.write_all(content.as_bytes())
        .await
        .map_err(|error| ConsoleError::unavailable(format!("could not stage input: {error}")))?;
    Ok(staged)
}

async fn read_bounded<R: AsyncRead + Unpin>(
    mut reader: R,
    limit: usize,
) -> std::io::Result<(Vec<u8>, bool)> {
    let mut output = Vec::with_capacity(16 * 1024);
    let mut buffer = [0_u8; 8192];
    let mut truncated = false;
    loop {
        let count = reader.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        let retained = limit.saturating_sub(output.len()).min(count);
        output.extend_from_slice(&buffer[..retained]);
        truncated |= retained < count;
    }
    Ok((output, truncated))
}

async fn run(body: &[u8]) -> Result<Value, ConsoleError> {
    let request: RunRequest = serde_json::from_slice(body)
        .map_err(|error| ConsoleError::bad_request(format!("invalid JSON: {error}")))?;
    validate(&request)?;
    let staged = if request.args.iter().any(|arg| arg == INPUT_PLACEHOLDER) {
        Some(stage_input(request.input.as_deref().unwrap_or_default()).await?)
    } else {
        None
    };
    let staged_args = staged.as_ref().map(|input| {
        request
            .args
            .iter()
            .map(|arg| {
                if arg == INPUT_PLACEHOLDER {
                    input.0.to_string_lossy().into_owned()
                } else {
                    arg.clone()
                }
            })
            .collect::<Vec<_>>()
    });
    let executable = std::env::current_exe().map_err(|error| {
        ConsoleError::unavailable(format!("could not resolve Stado binary: {error}"))
    })?;
    let mut child = Command::new(executable)
        .args(staged_args.as_deref().unwrap_or(&request.args))
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .env("STADO_DASHBOARD_CHILD", "1")
        // The server's local-store override is not the ordinary CLI client's
        // storage configuration. Children must use the configured object API.
        .env_remove("WC_STORAGE_BACKEND")
        .env_remove("WC_LOCAL_STORAGE_PATH")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| {
            ConsoleError::unavailable(format!("could not start Stado command: {error}"))
        })?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ConsoleError::unavailable("could not capture command stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ConsoleError::unavailable("could not capture command stderr"))?;
    let stdout_limit = if is_retained_log_request(&request.args) {
        MAX_RETAINED_LOG_OUTPUT_BYTES
    } else {
        MAX_OUTPUT_BYTES
    };
    let execution = async {
        let (stdout, stderr, status) = tokio::join!(
            read_bounded(stdout, stdout_limit),
            read_bounded(stderr, MAX_OUTPUT_BYTES),
            child.wait()
        );
        let stdout = stdout.map_err(|error| {
            ConsoleError::unavailable(format!("could not read command stdout: {error}"))
        })?;
        let stderr = stderr.map_err(|error| {
            ConsoleError::unavailable(format!("could not read command stderr: {error}"))
        })?;
        let status = status.map_err(|error| {
            ConsoleError::unavailable(format!("could not wait for Stado command: {error}"))
        })?;
        Ok::<_, ConsoleError>((stdout, stderr, status))
    };
    let ((stdout, stdout_truncated), (stderr, stderr_truncated), status) =
        tokio::time::timeout(Duration::from_secs(request.timeout_seconds), execution)
            .await
            .map_err(|_| {
                ConsoleError::unavailable(format!(
                    "command exceeded the {} second Desktop limit",
                    request.timeout_seconds
                ))
            })??;
    let stdout = String::from_utf8_lossy(&stdout).into_owned();
    let stderr = String::from_utf8_lossy(&stderr).into_owned();
    let structured = serde_json::from_str::<Value>(stdout.trim()).ok();
    Ok(
        json!({ "ok": status.success(), "exit_code": status.code(), "read_only": is_read_only(&request.args),
        "args": request.args, "stdout": stdout, "stderr": stderr, "stdout_truncated": stdout_truncated,
        "stderr_truncated": stderr_truncated, "structured": structured }),
    )
}

pub(super) async fn handle(request: &Request) -> Response {
    if request.path != "/api/operator/run"
        || request.header("x-stado-action") != Some("operator-command")
    {
        return send_json(403, &json!({"ok": false, "error": "forbidden"}));
    }
    let content_type = request
        .header("content-type")
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim();
    let content_length = request
        .header("content-length")
        .and_then(|value| value.parse::<usize>().ok());
    if content_type != "application/json"
        || request.header("transfer-encoding").is_some()
        || content_length != Some(request.body.len())
    {
        return send_json(
            400,
            &json!({"ok": false, "error": "invalid JSON request framing"}),
        );
    }
    match operator_auth::authorized(request).await {
        Ok(true) => {}
        Ok(false) => return send_json(401, &json!({"ok": false, "error": "unauthorized"})),
        Err(error) => return send_json(503, &json!({"ok": false, "error": error.to_string()})),
    }
    match run(&request.body).await {
        Ok(result) => send_json(200, &result),
        Err(error) => send_json(error.status, &json!({"ok": false, "error": error.message})),
    }
}
