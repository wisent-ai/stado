//! The `stado sandbox` verbs: each one Boat Public API v1 call through the
//! provider's client, its answer printed as JSON, and a Boat failure turned
//! into the CLI's error with the class it states.

use serde_json::{json, Value};

use crate::providers::r#box::types::HTTP_NOT_FOUND;
use crate::providers::r#box::{BoxError, BoxInfo};
use crate::providers::BoxProvider;

use super::{CmdError, CreateArgs, EnvArgs, ExecArgs, ReadArgs, WriteArgs};

/// The Boat provider as the deployment declares it: its client, and the
/// lifetime a sandbox created here gets unless the call states one.
pub(super) fn provider() -> Result<BoxProvider, CmdError> {
    BoxProvider::from_env().map_err(failure)
}

/// A Boat failure as the CLI reports it: a declaration Stado could not read
/// is `config`, an argument Boat's client refused is a usage error, a request
/// that got no readable answer is `infra_down`, and a refusal Boat answered
/// keeps its status, code and request id as details, stated `not_found` for
/// a sandbox Boat does not hold and `infra_down` when Boat says a retry can
/// help.
fn failure(error: BoxError) -> CmdError {
    match error {
        BoxError::Configuration(message) => CmdError::declaration(message),
        BoxError::Value(message) => CmdError::usage(message),
        BoxError::Transport(message) => CmdError::unreachable(message),
        BoxError::Api(api) => {
            let details = Value::Object(api.to_record());
            let sentence = api.to_string();
            let error = if api.status == HTTP_NOT_FOUND {
                CmdError::missing(sentence)
            } else if api.retryable {
                CmdError::unreachable(sentence)
            } else {
                CmdError::refused(sentence)
            };
            error.detailing(details)
        }
    }
}

/// One sandbox as the verbs print it.
fn sandbox(info: &BoxInfo) -> Value {
    json!({
        "id": info.box_id,
        "name": info.name,
        "state": info.state,
        "ip": info.ip,
        "url": info.url,
        "subdomain": info.subdomain,
        "createdAt": info.created_at,
        "updatedAt": info.updated_at,
        "archiveAfter": info.archive_after,
        "snapshotAvailable": info.snapshot_available,
        "snapshotCompletedAt": info.snapshot_completed_at,
        "lastSnapshotAttemptAt": info.last_snapshot_attempt_at,
        "lastSnapshotStatus": info.last_snapshot_status,
    })
}

fn print(value: &Value) -> Result<(), CmdError> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

pub(super) async fn list(provider: &BoxProvider) -> Result<(), CmdError> {
    let sandboxes = provider.client.list_boxes().await.map_err(failure)?;
    let rows: Vec<Value> = sandboxes.iter().map(sandbox).collect();
    print(&json!({ "sandboxes": rows }))
}

pub(super) async fn show(provider: &BoxProvider, id: &str) -> Result<(), CmdError> {
    let info = provider.client.get_box(id).await.map_err(failure)?;
    print(&sandbox(&info))
}

pub(super) async fn create(provider: &BoxProvider, args: &CreateArgs) -> Result<(), CmdError> {
    let ttl_seconds = match args.ttl_seconds {
        Some(stated) if stated.is_positive() => stated,
        Some(stated) => {
            return Err(CmdError::usage(format!(
                "--ttl-seconds {stated} is not a positive number of seconds a sandbox can live"
            )))
        }
        None => provider.ttl_seconds,
    };
    let info = provider
        .client
        .create_box(Some(ttl_seconds), args.no_env)
        .await
        .map_err(failure)?;
    print(&sandbox(&info))
}

pub(super) async fn exec(provider: &BoxProvider, args: &ExecArgs) -> Result<(), CmdError> {
    // The client sends no directory for an empty one, so without --cwd the
    // request names none and Boat chooses; a stated empty one is refused.
    let cwd = match args.cwd.as_deref() {
        Some("") => {
            return Err(CmdError::usage(
                "--cwd names no directory in the sandbox for the command to run in",
            ))
        }
        Some(directory) => directory,
        None => "",
    };
    let result = provider
        .client
        .execute_command(&args.id, &args.command, cwd)
        .await
        .map_err(failure)?;
    print(&json!({
        "success": result.success,
        "exitCode": result.exit_code,
        "signal": result.signal,
        "timedOut": result.timed_out,
        "stdout": result.stdout,
        "stderr": result.stderr,
        "stdoutTruncated": result.stdout_truncated,
        "stderrTruncated": result.stderr_truncated,
    }))?;
    if result.success {
        return Ok(());
    }
    let ended = match (result.exit_code, result.signal.is_empty(), result.timed_out) {
        (_, _, true) => String::from("ran past Boat's limit for a command"),
        (Some(code), _, false) => format!("exited with {code}"),
        (None, false, false) => format!("was ended by {}", result.signal),
        (None, true, false) => String::from("ended without an exit code or a signal"),
    };
    Err(CmdError::click(format!(
        "the command in sandbox {} {ended}",
        args.id
    )))
}

pub(super) async fn read(provider: &BoxProvider, args: &ReadArgs) -> Result<(), CmdError> {
    let answer = provider
        .client
        .read_file(&args.id, &args.path, &args.encoding)
        .await
        .map_err(failure)?;
    print(&Value::Object(answer))
}

pub(super) async fn write(provider: &BoxProvider, args: &WriteArgs) -> Result<(), CmdError> {
    let content = std::fs::read_to_string(&args.from).map_err(|error| {
        CmdError::click(format!(
            "cannot read {} to write it into sandbox {}: {error}",
            args.from.display(),
            args.id
        ))
        .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    let answer = provider
        .client
        .write_file(&args.id, &args.path, &content, &args.encoding)
        .await
        .map_err(failure)?;
    print(&Value::Object(answer))
}

pub(super) async fn fork(provider: &BoxProvider, args: &EnvArgs) -> Result<(), CmdError> {
    let info = provider
        .client
        .fork_box(&args.id, Some(args.no_env))
        .await
        .map_err(failure)?;
    print(&sandbox(&info))
}

pub(super) async fn stop(provider: &BoxProvider, id: &str) -> Result<(), CmdError> {
    let answer = provider.client.stop_box(id).await.map_err(failure)?;
    print(&Value::Object(answer))
}

pub(super) async fn resume(provider: &BoxProvider, args: &EnvArgs) -> Result<(), CmdError> {
    let answer = provider
        .client
        .resume_box(&args.id, Some(args.no_env))
        .await
        .map_err(failure)?;
    print(&Value::Object(answer))
}

pub(super) async fn delete(provider: &BoxProvider, id: &str) -> Result<(), CmdError> {
    provider.client.delete_box(id).await.map_err(failure)?;
    print(&json!({ "deleted": id }))
}
