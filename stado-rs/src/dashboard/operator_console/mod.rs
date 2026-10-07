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
use std::num::NonZeroUsize;
use std::sync::atomic::AtomicU64;

use super::{operator_auth, send_json, Request, Response};
use families::{is_read_only, ALLOWED_FAMILIES};

/// One declaration bounds both the command API and interactive attachments.
#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Limits {
    pub(crate) argument_count: NonZeroUsize,
    pub(crate) argument_bytes: NonZeroUsize,
    pub(crate) input_bytes: NonZeroUsize,
    pub(crate) request_bytes: NonZeroUsize,
}
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

fn validate(request: &RunRequest, limits: Limits) -> Result<(), ConsoleError> {
    let Some(family) = request.args.first() else {
        return Err(ConsoleError::bad_request(
            "args must contain a command family",
        ));
    };
    if request.args.len() > limits.argument_count.get() {
        return Err(ConsoleError::bad_request(format!(
            "args contains {} values; dashboard.request_limits.operator_console.argument_count permits {}",
            request.args.len(),
            limits.argument_count
        )));
    }
    if let Some(argument) = request
        .args
        .iter()
        .find(|arg| arg.is_empty() || arg.len() > limits.argument_bytes.get() || arg.contains('\0'))
    {
        return Err(ConsoleError::bad_request(format!(
            "argument has {} bytes; arguments must be non-empty and without NUL bytes; dashboard.request_limits.operator_console.argument_bytes permits {}",
            argument.len(),
            limits.argument_bytes
        )));
    }
    if !ALLOWED_FAMILIES.contains(&family.as_str()) {
        return Err(ConsoleError::forbidden(format!(
            "command family {family:?} is not available in the Desktop API"
        )));
    }
    for (name, value) in [
        ("input", request.input.as_ref()),
        ("stdin", request.stdin.as_ref()),
    ] {
        if let Some(value) = value.filter(|value| value.len() > limits.input_bytes.get()) {
            return Err(ConsoleError::bad_request(format!(
                "{name} has {} bytes; dashboard.request_limits.operator_console.input_bytes permits {}",
                value.len(),
                limits.input_bytes
            )));
        }
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
    match execute::run(&request.body, request.console_limits).await {
        Ok(result) => send_json(STATUS_OK, &result),
        Err(error) => send_json(error.status, &json!({"ok": false, "error": error.message})),
    }
}
