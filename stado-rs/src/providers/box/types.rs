//! Bounded Box API value objects and redacted failures.
//!
//! Port of `stado/providers/box/_types.py`. The frozen Python dataclasses
//! map to plain structs with public fields; the exception trio maps to
//! [`BoxError`] variants, with the structured, redacted API failure kept as
//! a standalone [`BoxApiError`] payload (status/code/message/request_id/
//! retryable) so callers can match on 404s and retryability exactly like
//! the Python `except BoxAPIError as exc: exc.status == 404` sites.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value};

/// Python `DEFAULT_BOX_API_URL`.
pub const DEFAULT_BOX_API_URL: &str = "https://ascii.dev/api/box/v1";
/// Python `DEFAULT_TIMEOUT_SECONDS`.
pub const DEFAULT_TIMEOUT_SECONDS: f64 = 70.0;
/// Python `MAX_JSON_BYTES`.
pub const MAX_JSON_BYTES: usize = 65536;
/// Python `HTTP_NOT_FOUND`.
pub const HTTP_NOT_FOUND: u16 = 404;
/// Python `TRANSIENT_HTTP`: statuses the caller may retry.
pub const TRANSIENT_HTTP: [u16; 5] = [429, 500, 502, 503, 504];

/// Python `BOX_ID_PATTERN` (`fullmatch` semantics — the pattern is anchored).
pub fn box_id_pattern() -> &'static Regex {
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^bx_[23456789abcdefghjkmnpqrstuvwxyz]{8}$").expect("static regex compiles")
    });
    &RE
}

fn box_key_pattern() -> &'static Regex {
    static RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"box_[A-Za-z0-9_-]+").expect("static regex compiles"));
    &RE
}

fn url_token_pattern() -> &'static Regex {
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)([?&](?:_token|token|key|access_token)=)[^&\s]+")
            .expect("static regex compiles")
    });
    &RE
}

fn authorization_pattern() -> &'static Regex {
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)authorization\s*[:=]\s*[^,;\s]+").expect("static regex compiles")
    });
    &RE
}

/// Box-layer error. Python raises `BoxConfigurationError` for local config
/// problems, `BoxTransportError` for bounded network/response failures,
/// `BoxAPIError` for structured redacted API failures, and `ValueError` for
/// client-side argument validation.
#[derive(Debug, thiserror::Error)]
pub enum BoxError {
    /// Python `BoxConfigurationError`.
    #[error("{0}")]
    Configuration(String),
    /// Python `BoxTransportError`.
    #[error("{0}")]
    Transport(String),
    /// Python `ValueError` from argument validation (invalid box id, empty
    /// command, out-of-bounds timeout, ...).
    #[error("{0}")]
    Value(String),
    /// Python `BoxAPIError`.
    #[error("{0}")]
    Api(#[from] BoxApiError),
}

impl BoxError {
    pub(crate) fn configuration(message: impl Into<String>) -> Self {
        BoxError::Configuration(message.into())
    }

    pub(crate) fn transport(message: impl Into<String>) -> Self {
        BoxError::Transport(message.into())
    }

    pub(crate) fn value(message: impl Into<String>) -> Self {
        BoxError::Value(message.into())
    }
}

/// Structured, redacted Box API failure (Python `BoxAPIError`). Every text
/// field passed through [`safe_text`] at construction, so a key or token
/// embedded by the server never reaches logs.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub struct BoxApiError {
    pub status: u16,
    pub code: String,
    pub message: String,
    pub request_id: String,
    pub retryable: bool,
}

impl BoxApiError {
    /// Python `BoxAPIError.__init__`.
    pub fn new(status: u16, code: &str, message: &str, request_id: &str, retryable: bool) -> Self {
        BoxApiError {
            status,
            code: safe_text(code, "box_error"),
            message: safe_text(message, "Box API request failed"),
            request_id: safe_text(request_id, ""),
            retryable,
        }
    }

    /// Python `BoxAPIError.to_record` (dict key order preserved).
    pub fn to_record(&self) -> Map<String, Value> {
        Map::from_iter([
            ("status".to_string(), Value::from(self.status)),
            ("code".to_string(), Value::from(self.code.clone())),
            ("message".to_string(), Value::from(self.message.clone())),
            (
                "request_id".to_string(),
                Value::from(self.request_id.clone()),
            ),
            ("retryable".to_string(), Value::from(self.retryable)),
        ])
    }
}

impl std::fmt::Display for BoxApiError {
    /// Python `BoxAPIError.__str__`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let suffix = if self.request_id.is_empty() {
            String::new()
        } else {
            format!(" request_id={}", self.request_id)
        };
        write!(
            f,
            "Box API HTTP {} [{}]: {}{}",
            self.status, self.code, self.message, suffix
        )
    }
}

/// Python `BoxInfo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxInfo {
    pub box_id: String,
    pub name: String,
    pub state: String,
    pub ip: String,
    pub url: String,
    pub subdomain: String,
    pub created_at: String,
    pub updated_at: String,
    pub archive_after: String,
    pub snapshot_available: bool,
    pub snapshot_completed_at: String,
    pub last_snapshot_attempt_at: String,
    pub last_snapshot_status: String,
}

/// Python `BoxLimits`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxLimits {
    pub can_start: bool,
    pub active_boxes: i64,
    pub max_active_boxes: i64,
    pub billing_status: String,
    pub blocked_reason: String,
    pub credit_balance_seconds: i64,
}

/// Python `BoxCommandResult`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxCommandResult {
    pub success: bool,
    pub exit_code: Option<i64>,
    pub signal: String,
    pub stdout: String,
    pub stderr: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub timed_out: bool,
}

/// Python `BoxPromptRun`; `raw` is the unmodified `promptRun` dict.
#[derive(Debug, Clone, PartialEq)]
pub struct BoxPromptRun {
    pub prompt_id: String,
    pub status: String,
    pub done: bool,
    pub raw: Map<String, Value>,
}

/// Python `BoxEventPage`.
#[derive(Debug, Clone, PartialEq)]
pub struct BoxEventPage {
    pub events: Vec<Map<String, Value>>,
    pub next_cursor: String,
    pub has_more: bool,
}

/// Python `safe_text`: single-line text with box keys, URL tokens and
/// Authorization headers redacted, truncated to `limit` characters.
/// An empty `value` falls back to `default_text` (Python `value or default`).
pub fn safe_text(value: &str, default_text: &str) -> String {
    safe_text_limited(value, default_text, 512)
}

/// [`safe_text`] with an explicit limit (Python `limit=512` default).
pub fn safe_text_limited(value: &str, default_text: &str, limit: usize) -> String {
    let text = if value.is_empty() {
        default_text
    } else {
        value
    };
    let text = text.replace(['\r', '\n'], " ");
    let text = box_key_pattern().replace_all(&text, "[REDACTED]");
    let text = url_token_pattern().replace_all(&text, "$1[REDACTED]");
    let text = authorization_pattern().replace_all(&text, "Authorization=[REDACTED]");
    // Python text[:limit] slices code points, not bytes.
    text.chars().take(limit).collect()
}

/// Python `required_dict`.
pub fn required_dict(value: Value, context: &str) -> Result<Map<String, Value>, BoxError> {
    match value {
        Value::Object(map) => Ok(map),
        _ => Err(BoxError::transport(format!(
            "Box {context} response is not an object"
        ))),
    }
}

/// Python truthiness of the JSON value (None/False/0/""/[]/{} are falsy).
pub(crate) fn py_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Python `str(value)`: strings pass through, null becomes "None",
/// booleans become "True"/"False", containers their JSON form.
pub(crate) fn py_str(value: &Value) -> String {
    match value {
        Value::Null => "None".to_string(),
        Value::Bool(b) => if *b { "True" } else { "False" }.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Python `str(value.get(key) or "")`: falsy values map to "".
pub(crate) fn jstr(value: Option<&Value>) -> String {
    match value {
        Some(v) if py_truthy(v) => py_str(v),
        _ => String::new(),
    }
}

/// Python `int(value.get(key) or "<default>")`: falsy -> default, floats
/// truncate, strings parse (Python `int()` strips whitespace).
pub(crate) fn jint_or(value: Option<&Value>, default: i64) -> Result<i64, BoxError> {
    let Some(value) = value.filter(|v| py_truthy(v)) else {
        return Ok(default);
    };
    match value {
        Value::Number(n) => Ok(n.as_f64().unwrap_or(0.0) as i64),
        Value::String(s) => s.trim().parse::<i64>().map_err(|_| {
            BoxError::value(format!(
                "invalid literal for int() with base 10: {:?}",
                s.trim()
            ))
        }),
        other => Err(BoxError::value(format!(
            "int() argument must be a string or a number, not {}",
            py_str(other)
        ))),
    }
}

/// Python `bool(value.get(key))`.
pub(crate) fn jbool(value: Option<&Value>) -> bool {
    value.is_some_and(py_truthy)
}

/// First truthy value rendered with [`py_str`], else `fallback` (Python
/// `v1 or v2 or "fallback"` chains in error-payload parsing).
pub(crate) fn first_truthy_str(values: &[Option<&Value>], fallback: &str) -> String {
    for value in values.iter().flatten() {
        if py_truthy(value) {
            return py_str(value);
        }
    }
    fallback.to_string()
}

/// Python `parse_box_info`: unwrap the `"box"` envelope when present,
/// require a pattern-conforming id, default every other field.
pub fn parse_box_info(payload: &Map<String, Value>) -> Result<BoxInfo, BoxError> {
    let box_value = payload
        .get("box")
        .cloned()
        .unwrap_or(Value::Object(payload.clone()));
    let boxed = required_dict(box_value, "box")?;
    let box_id = jstr(boxed.get("id"));
    if !box_id_pattern().is_match(&box_id) {
        return Err(BoxError::transport(
            "Box response contains an invalid box id",
        ));
    }
    Ok(BoxInfo {
        box_id,
        name: jstr(boxed.get("name")),
        state: jstr(boxed.get("state")),
        ip: jstr(boxed.get("ip")),
        url: jstr(boxed.get("url")),
        subdomain: jstr(boxed.get("subdomain")),
        created_at: jstr(boxed.get("createdAt")),
        updated_at: jstr(boxed.get("updatedAt")),
        archive_after: jstr(boxed.get("archiveAfter")),
        snapshot_available: jbool(boxed.get("snapshotAvailable")),
        snapshot_completed_at: jstr(boxed.get("snapshotCompletedAt")),
        last_snapshot_attempt_at: jstr(boxed.get("lastSnapshotAttemptAt")),
        last_snapshot_status: jstr(boxed.get("lastSnapshotStatus")),
    })
}

