//! Dispatch: CLI resolution, the subprocess call behind every tool, and
//! the JSON-RPC method routing that reaches them.

use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::primitives::procutil::{run_capture, Capture};

use super::protocol::{
    error_response, ToolError, CODE_INTERNAL_ERROR, CODE_METHOD_NOT_FOUND, JSONRPC_VERSION,
    PROTOCOL_VERSION, SUBPROCESS_TIMEOUT_SECONDS,
};
use super::tools::{tool_by_name, TOOLS};

/// Resolve how to invoke the stado CLI (see module docs for the deviation
/// from Python `_stado_argv`): `$STADO_BIN` -> sibling of the running
/// executable -> `stado` on PATH.
pub fn stado_argv() -> Vec<String> {
    if let Ok(bin) = std::env::var("STADO_BIN") {
        if !bin.is_empty() {
            return vec![bin];
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("stado");
            if sibling.exists() {
                return vec![sibling.to_string_lossy().into_owned()];
            }
        }
    }
    vec!["stado".to_string()]
}

/// Invoke the stado CLI in a subprocess; return stdout, or a [`ToolError`]
/// (Python `_run`).
fn run(cli_tokens: &[&str], extra: &[String]) -> Result<String, ToolError> {
    let mut argv = stado_argv();
    argv.extend(cli_tokens.iter().map(|token| token.to_string()));
    argv.extend(extra.iter().cloned());
    let capture = run_capture(&argv, Duration::from_secs(SUBPROCESS_TIMEOUT_SECONDS))
        .map_err(|err| ToolError::internal(format!("stado CLI not found: {err}")))?;
    match capture {
        Capture::TimedOut { .. } => {
            let rendered: Vec<String> =
                argv.iter().map(|a| crate::models::py_str_repr(a)).collect();
            Err(ToolError::internal(format!(
                "stado CLI timed out: Command '[{}]' timed out after {SUBPROCESS_TIMEOUT_SECONDS} seconds",
                rendered.join(", ")
            )))
        }
        Capture::Completed { rc, stdout, stderr } => {
            if rc != 0 {
                let detail = if stderr.trim().is_empty() {
                    stdout.trim()
                } else {
                    stderr.trim()
                };
                return Err(ToolError::internal(if detail.is_empty() {
                    format!("stado {} exited nonzero", cli_tokens.join(" "))
                } else {
                    detail.to_string()
                }));
            }
            Ok(stdout)
        }
    }
}

/// Python `_text_result`.
fn text_result(text: String) -> Value {
    json!({"content": [{"type": "text", "text": text}]})
}

/// Python's truthiness for a JSON argument value (used for the
/// `not value` checks in `call_tool`).
fn python_falsy(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Bool(b) => !b,
        Value::Number(n) => n.as_f64() == Some(0.0),
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
    }
}

/// Python `str(value)` for a JSON argument value.
fn python_str(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => "None".to_string(),
        Value::Bool(true) => "True".to_string(),
        Value::Bool(false) => "False".to_string(),
        other => other.to_string(),
    }
}

/// Dispatch a read-only tool by name (Python `call_tool`); unknown names
/// raise a -32601 ToolError.
pub fn call_tool(name: &str, args: &Map<String, Value>) -> Result<Value, ToolError> {
    let Some(tool) = tool_by_name(name) else {
        return Err(ToolError {
            code: CODE_METHOD_NOT_FOUND,
            message: format!("unknown tool: {name}"),
        });
    };
    let mut extra: Vec<String> = Vec::new();
    if let Some(arg) = &tool.arg {
        let value = args.get(arg.name);
        let falsy = value.map(python_falsy).unwrap_or(true);
        if arg.required && falsy {
            return Err(ToolError::internal(format!(
                "missing required argument: {}",
                arg.name
            )));
        }
        if let Some(value) = value {
            if !falsy {
                if let Some(flag) = arg.flag {
                    extra.push(flag.to_string());
                }
                extra.push(python_str(value));
            }
        }
    }
    Ok(text_result(run(tool.cli, &extra)?))
}

/// Installed package version for `serverInfo` (Python
/// `_server_version()` reads importlib.metadata; the crate version tracks
/// the Python package version).
fn server_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Route one JSON-RPC request (Python `handle`). Returns the response to
/// emit, or `None` when the request is a notification (no `id` key) or
/// carries no usable `method`.
pub fn handle(request: &Value) -> Option<Value> {
    let obj = request.as_object()?;
    let method_value = obj.get("method")?;
    // Python: `if not method: return` — missing/null/"" (or false) methods
    // get no response at all.
    let method_str = method_value.as_str();
    if method_value.is_null() || method_str == Some("") || method_value == &Value::Bool(false) {
        return None;
    }
    // A request without an `id` key is a notification: never answer it.
    let rid = obj.get("id")?.clone();
    let result = match method_str {
        Some("initialize") => json!({
            "jsonrpc": JSONRPC_VERSION,
            "id": rid,
            "result": {
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "stado", "version": server_version()},
            },
        }),
        Some("ping") => json!({"jsonrpc": JSONRPC_VERSION, "id": rid, "result": {}}),
        Some("tools/list") => {
            json!({"jsonrpc": JSONRPC_VERSION, "id": rid, "result": {"tools": Value::Array(TOOLS.clone())}})
        }
        Some("tools/call") => {
            let params = obj.get("params").and_then(Value::as_object);
            let empty = Map::new();
            let params = params.unwrap_or(&empty);
            let name = params.get("name");
            let name = match name.and_then(Value::as_str) {
                Some(name) => name,
                None => {
                    return Some(error_response(
                        &rid,
                        CODE_INTERNAL_ERROR,
                        "params.name must be a string",
                    ));
                }
            };
            let args = params
                .get("arguments")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            match call_tool(name, &args) {
                Ok(result) => json!({"jsonrpc": JSONRPC_VERSION, "id": rid, "result": result}),
                Err(err) => error_response(&rid, err.code, &err.message),
            }
        }
        Some(other) => error_response(
            &rid,
            CODE_METHOD_NOT_FOUND,
            &format!("method not found: {other}"),
        ),
        None => error_response(
            &rid,
            CODE_METHOD_NOT_FOUND,
            &format!("method not found: {method_value}"),
        ),
    };
    Some(result)
}
