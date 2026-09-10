//! Bounded native Desktop access to the canonical CLI implementation.
//! Requests contain argv, never a shell command. The retired HTML catalog is
//! not needed by native clients and is intentionally not restored.
//!
//! Kept in three parts small enough to read: this file owns the route, the
//! request shape and validation; [`families`] owns which commands may be
//! reached and which of them only read; [`execute`] owns running one.

mod execute;
mod families;
pub(crate) mod stream;

use serde::Deserialize;
use serde_json::json;
use std::sync::atomic::AtomicU64;

use super::{operator_auth, send_json, Request, Response};
use families::{is_read_only, ALLOWED_FAMILIES};

const MAX_ARGUMENTS: usize = 96;
const MAX_ARGUMENT_BYTES: usize = 4096;
const MAX_INPUT_BYTES: usize = 1024 * 1024;
pub(super) const MAX_REQUEST_BYTES: usize = MAX_INPUT_BYTES + 128 * 1024;
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
// A fixed one-hour native log can exceed the ordinary command preview. Keep
// its JSON receipt intact for Desktop while retaining a bounded capture.
const MAX_RETAINED_LOG_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
const MAX_TIMEOUT_SECONDS: u64 = 300;
// Inventory and explicit scratch removal can traverse whole filesystems.
const MAX_SPACE_COMMAND_SECONDS: u64 = 1200;
const MUTATION_CONFIRMATION: &str = "RUN_MUTATION";
const INPUT_PLACEHOLDER: &str = "$INPUT";
static INPUT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// The statuses this route answers with, one name per condition.
///
/// A bare `400` beside a message says only that something is a client error;
/// the name says which of the three refusals a reader is looking at, and the
/// three are answered from four different places in this file.
const STATUS_OK: u16 = 200;
const STATUS_BAD_REQUEST: u16 = 400;
const STATUS_UNAUTHORIZED: u16 = 401;
const STATUS_FORBIDDEN: u16 = 403;
const STATUS_UNAVAILABLE: u16 = 503;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunRequest {
    args: Vec<String>,
    #[serde(default)]
    input: Option<String>,
    #[serde(default)]
    stdin: Option<String>,
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
            status: STATUS_BAD_REQUEST,
            message: message.into(),
        }
    }
    fn forbidden(message: impl Into<String>) -> Self {
        Self {
            status: STATUS_FORBIDDEN,
            message: message.into(),
        }
    }
    fn unavailable(message: impl Into<String>) -> Self {
        Self {
            status: STATUS_UNAVAILABLE,
            message: message.into(),
        }
    }
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
    let limit = if matches!(
        request.args.first().map(String::as_str),
        Some("space" | "workdirs")
    ) {
        MAX_SPACE_COMMAND_SECONDS
    } else {
        MAX_TIMEOUT_SECONDS
    };
    if request.timeout_seconds == 0 || request.timeout_seconds > limit {
        return Err(ConsoleError::bad_request(format!(
            "timeout_seconds must be between 1 and {limit}"
        )));
    }
    if request.input.as_ref().map_or(0, String::len) > MAX_INPUT_BYTES {
        return Err(ConsoleError::bad_request(format!(
            "input exceeds the {MAX_INPUT_BYTES}-byte limit"
        )));
    }
    if request.stdin.as_ref().map_or(0, String::len) > MAX_INPUT_BYTES {
        return Err(ConsoleError::bad_request(format!(
            "stdin exceeds the {MAX_INPUT_BYTES}-byte limit"
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

pub(super) async fn handle(request: &Request) -> Response {
    if request.path != "/api/operator/run"
        || request.header("x-stado-action") != Some("operator-command")
    {
        return send_json(
            STATUS_FORBIDDEN,
            &json!({"ok": false, "error": "forbidden"}),
        );
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
            STATUS_BAD_REQUEST,
            &json!({"ok": false, "error": "invalid JSON request framing"}),
        );
    }
    match operator_auth::authorized(request).await {
        Ok(true) => {}
        Ok(false) => {
            return send_json(
                STATUS_UNAUTHORIZED,
                &json!({"ok": false, "error": "unauthorized"}),
            )
        }
        Err(error) => {
            return send_json(
                STATUS_UNAVAILABLE,
                &json!({"ok": false, "error": error.to_string()}),
            )
        }
    }
    match execute::run(&request.body).await {
        Ok(result) => send_json(STATUS_OK, &result),
        Err(error) => send_json(error.status, &json!({"ok": false, "error": error.message})),
    }
}
