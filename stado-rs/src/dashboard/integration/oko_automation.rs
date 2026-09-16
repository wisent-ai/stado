//! Fixed Oko automation operations. No caller-supplied command or executable.
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{oko, HandlerError, HandlerResult};

const RESPONSE_LIMIT: usize = 4 * 1024 * 1024;
const DIAGNOSTIC_CHARACTERS: usize = 8192;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Host {
    host_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Context {
    host_id: String,
    query: Option<String>,
    database: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum RoutineAction {
    Context,
    Autonomy,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Create {
    host_id: String,
    id: String,
    name: String,
    action: RoutineAction,
    cron: String,
    time_zone: String,
    query: Option<String>,
    database: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Routine {
    host_id: String,
    schedule_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Run {
    host_id: String,
    schedule_id: String,
    retry_token: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Inspect {
    host_id: String,
    job_id: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Mode {
    Disabled,
    Experimental,
    Full,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    host_id: String,
    mode: Mode,
}

fn decode<T: DeserializeOwned>(body: &[u8]) -> Result<T, HandlerError> {
    serde_json::from_slice(body).map_err(|_| HandlerError::BadRequest)
}
fn words(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_owned()).collect()
}
fn identity(value: &str, prefix: &str) -> Result<(), HandlerError> {
    if value.strip_prefix(prefix).is_none_or(|suffix| {
        suffix.is_empty()
            || !suffix
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    }) {
        return Err(HandlerError::BadRequest);
    }
    Ok(())
}
fn optional(arguments: &mut Vec<String>, flag: &str, value: Option<String>) {
    if let Some(value) = value {
        arguments.extend([flag.to_owned(), value]);
    }
}

pub(super) fn supports(action: &str) -> bool {
    matches!(
        action,
        "context-snapshot"
            | "slack-status"
            | "autonomy-status"
            | "autonomy-set"
            | "routines-list"
            | "routines-create"
            | "routines-show"
            | "routines-pause"
            | "routines-resume"
            | "routines-remove"
            | "routines-run"
            | "routines-inspect"
    )
}

fn request(action: &str, body: &[u8]) -> Result<(String, Vec<String>), HandlerError> {
    match action {
        "context-snapshot" => {
            let value: Context = decode(body)?;
            let mut argv = words(&["oko-cli", "context", "snapshot"]);
            optional(&mut argv, "--query", value.query);
            optional(&mut argv, "--db", value.database);
            Ok((value.host_id, argv))
        }
        "slack-status" | "autonomy-status" | "routines-list" => {
            let value: Host = decode(body)?;
            let argv = match action {
                "slack-status" => words(&["oko-cli", "slack", "status", "--json"]),
                "autonomy-status" => words(&["oko-cli", "autonomy", "status"]),
                _ => words(&["oko-cli", "routines", "list", "--json"]),
            };
            Ok((value.host_id, argv))
        }
        "autonomy-set" => {
            let value: Policy = decode(body)?;
            let argv = match value.mode {
                Mode::Disabled => words(&["oko-cli", "autonomy", "disable"]),
                Mode::Experimental => {
                    words(&["oko-cli", "autonomy", "enable", "--mode", "experimental"])
                }
                Mode::Full => words(&["oko-cli", "autonomy", "enable", "--mode", "full"]),
            };
            Ok((value.host_id, argv))
        }
        "routines-create" => {
            let value: Create = decode(body)?;
            uuid::Uuid::parse_str(&value.id).map_err(|_| HandlerError::BadRequest)?;
            let mut argv = words(&[
                "oko-cli",
                "routines",
                "create",
                "--id",
                &value.id,
                "--name",
                &value.name,
                "--action",
                match value.action {
                    RoutineAction::Context => "context",
                    RoutineAction::Autonomy => "autonomy",
                },
                "--cron",
                &value.cron,
                "--tz",
                &value.time_zone,
                "--host",
                &value.host_id,
            ]);
            optional(&mut argv, "--query", value.query);
            optional(&mut argv, "--db", value.database);
            Ok((value.host_id, argv))
        }
        "routines-show" | "routines-pause" | "routines-resume" | "routines-remove" => {
            let value: Routine = decode(body)?;
            identity(&value.schedule_id, "sch-")?;
            let verb = action
                .strip_prefix("routines-")
                .ok_or(HandlerError::BadRequest)?;
            Ok((
                value.host_id,
                words(&["oko-cli", "routines", verb, &value.schedule_id, "--json"]),
            ))
        }
        "routines-run" => {
            let value: Run = decode(body)?;
            identity(&value.schedule_id, "sch-")?;
            if value.retry_token.trim().is_empty() {
                return Err(HandlerError::BadRequest);
            }
            Ok((
                value.host_id,
                words(&[
                    "oko-cli",
                    "routines",
                    "run",
                    &value.schedule_id,
                    "--retry-token",
                    &value.retry_token,
                    "--json",
                ]),
            ))
        }
        "routines-inspect" => {
            let value: Inspect = decode(body)?;
            identity(&value.job_id, "job-")?;
            Ok((
                value.host_id,
                words(&["oko-cli", "routines", "inspect", &value.job_id, "--json"]),
            ))
        }
        _ => Err(HandlerError::BadRequest),
    }
}

pub(super) async fn handle(action: &str, body: &[u8]) -> HandlerResult {
    let (host, arguments) = request(action, body)?;
    if arguments.iter().any(|argument| argument.contains('\0')) {
        return Err(HandlerError::BadRequest);
    }
    let target = oko::target(&host).await?;
    let runner = crate::deploy::production_runner();
    let program: Vec<&str> = arguments.iter().map(String::as_str).collect();
    let output = crate::deploy::host_channel::run_program(&target, &program, &runner)
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    if output.stdout.len() > RESPONSE_LIMIT {
        return Err(HandlerError::ResponseTooLarge);
    }
    let data = serde_json::from_str::<Value>(&output.stdout);
    if output.ok() && data.is_err() {
        return Err(HandlerError::UpstreamFailure);
    }
    // A successful transport is not a successful owner operation. Retain its
    // structured refusal (e.g. Slack readiness) and bounded actual diagnostic.
    Ok(
        json!({"hostId": host, "operation": action, "exitStatus": output.code,
        "data": data.unwrap_or(Value::Null),
        "diagnostic": output.stderr.chars().take(DIAGNOSTIC_CHARACTERS).collect::<String>()}),
    )
}
