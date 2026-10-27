//! Dispatch: CLI resolution, the subprocess call behind every tool, and
//! the JSON-RPC method routing that reaches them.

use serde_json::{json, Map, Value};

use crate::primitives::procutil::{run_capture, Capture};

use super::protocol::{
    error_response, ToolError, CODE_INTERNAL_ERROR, CODE_METHOD_NOT_FOUND, JSONRPC_VERSION,
    PROTOCOL_VERSION,
};
use super::tools::{tool_by_name, TOOLS};

/// Resolve how to invoke the stado CLI: `$STADO_BIN` -> the running
/// executable, which is `stado` itself -> `stado` on PATH.
pub fn stado_argv() -> Vec<String> {
    if let Ok(bin) = std::env::var("STADO_BIN") {
        if !bin.is_empty() {
            return vec![bin];
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        return vec![exe.to_string_lossy().into_owned()];
    }
    vec!["stado".to_string()]
}

/// Invoke the stado CLI in a subprocess; return stdout, or a [`ToolError`]
/// (Python `_run`).
fn run(cli_tokens: &[&str], extra: &[String]) -> Result<String, ToolError> {
    let mut argv = stado_argv();
    argv.extend(cli_tokens.iter().map(|token| token.to_string()));
    argv.extend(extra.iter().cloned());
    let Capture { rc, stdout, stderr } = run_capture(&argv).map_err(|err| {
        ToolError::internal(format!(
            "stado {} did not run to an exit: {err}",
            cli_tokens.join(" ")
        ))
    })?;
    if std::num::NonZeroI32::new(rc).is_some() {
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

/// Python `_text_result`.
fn text_result(text: String) -> Value {
    json!({"content": [{"type": "text", "text": text}]})
}

/// A tool argument as its schema declares every one: text. Absent, null or
/// empty text gives none; any other type is refused naming the argument.
/// Python truthiness read `false` and `0` as no argument and `str()` passed
/// `True` or `None` to the command line as if the caller had written them.
fn argument_text(name: &str, value: Option<&Value>) -> Result<Option<String>, ToolError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) if text.is_empty() => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(other) => Err(ToolError::internal(format!(
            "argument {name} must be a string, as the tool's schema declares; it was {other}"
        ))),
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
        match argument_text(arg.name, args.get(arg.name))? {
            Some(text) => {
                if let Some(flag) = arg.flag {
                    extra.push(flag.to_string());
                }
                extra.push(text);
            }
            None if arg.required => {
                return Err(ToolError::internal(format!(
                    "missing required argument: {}",
                    arg.name
                )));
            }
            None => {}
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
            let Some(params) = obj.get("params").and_then(Value::as_object) else {
                return Some(error_response(
                    &rid,
                    CODE_INTERNAL_ERROR,
                    "params must be an object naming the tool",
                ));
            };
            let name = match params.get("name").and_then(Value::as_str) {
                Some(name) => name,
                None => {
                    return Some(error_response(
                        &rid,
                        CODE_INTERNAL_ERROR,
                        "params.name must be a string",
                    ));
                }
            };
            // A call may pass no arguments at all; arguments of another
            // shape are refused rather than read as none.
            let args = match params.get("arguments") {
                None | Some(Value::Null) => Map::new(),
                Some(Value::Object(arguments)) => arguments.clone(),
                Some(other) => {
                    return Some(error_response(
                        &rid,
                        CODE_INTERNAL_ERROR,
                        &format!("params.arguments must be an object, not {other}"),
                    ));
                }
            };
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
