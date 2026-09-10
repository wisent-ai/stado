//! Running one validated argv as a child of this process, with bounded
//! output and a staged input file that never survives the request.

use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::Ordering;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use super::families::{is_read_only, is_retained_log_request};
use super::{
    validate, ConsoleError, RunRequest, INPUT_PLACEHOLDER, INPUT_SEQUENCE, MAX_OUTPUT_BYTES,
    MAX_RETAINED_LOG_OUTPUT_BYTES,
};

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

pub(super) async fn run(body: &[u8]) -> Result<Value, ConsoleError> {
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
        .stdin(if request.stdin.is_some() { Stdio::piped() } else { Stdio::null() })
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
    let stdin = child.stdin.take();
    let stdout_limit = if is_retained_log_request(&request.args) {
        MAX_RETAINED_LOG_OUTPUT_BYTES
    } else {
        MAX_OUTPUT_BYTES
    };
    let execution = async {
        let (stdout, stderr, status, input) = tokio::join!(
            read_bounded(stdout, stdout_limit),
            read_bounded(stderr, MAX_OUTPUT_BYTES),
            child.wait(),
            async {
                if let (Some(mut pipe), Some(content)) = (stdin, request.stdin.as_deref()) {
                    pipe.write_all(content.as_bytes()).await?;
                    pipe.shutdown().await?;
                }
                Ok::<_, std::io::Error>(())
            }
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
        let stdin_error = input.err().map(|error| format!("could not write command stdin: {error}"));
        Ok::<_, ConsoleError>((stdout, stderr, status, stdin_error))
    };
    let ((stdout, stdout_truncated), (stderr, stderr_truncated), status, stdin_error) =
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
        json!({ "ok": status.success() && stdin_error.is_none(), "exit_code": status.code(), "read_only": is_read_only(&request.args),
        "args": request.args, "stdout": stdout, "stderr": stderr, "stdout_truncated": stdout_truncated,
        "stderr_truncated": stderr_truncated, "stdin_error": stdin_error, "structured": structured }),
    )
}
