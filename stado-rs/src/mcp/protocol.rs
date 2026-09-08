//! JSON-RPC protocol shapes: the wire constants, the tool-failure type,
//! and the error-response envelope.

use serde_json::{json, Value};

/// Spec-mandated MCP protocol version (not a tunable).
pub const PROTOCOL_VERSION: &str = "2024-11-05";
/// JSON-RPC envelope version.
pub const JSONRPC_VERSION: &str = "2.0";
/// JSON-RPC parse error.
pub const CODE_PARSE_ERROR: i64 = -32700;
/// JSON-RPC method not found (also used for unknown tools).
pub const CODE_METHOD_NOT_FOUND: i64 = -32601;
/// Python `CODE_INTERNAL_ERROR` — -32000 as written, not the spec's -32603.
pub const CODE_INTERNAL_ERROR: i64 = -32000;
/// Subprocess timeout for one CLI dispatch (Python
/// `SUBPROCESS_TIMEOUT_SECONDS`).
pub const SUBPROCESS_TIMEOUT_SECONDS: u64 = 600;

/// A tool failure carrying the JSON-RPC error code to report (Python
/// `ToolError`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolError {
    pub code: i64,
    pub message: String,
}

impl ToolError {
    pub(super) fn internal(message: impl Into<String>) -> Self {
        Self {
            code: CODE_INTERNAL_ERROR,
            message: message.into(),
        }
    }
}

/// Python `_error`.
pub(super) fn error_response(rid: &Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": JSONRPC_VERSION,
        "id": rid.clone(),
        "error": {"code": code, "message": message},
    })
}
