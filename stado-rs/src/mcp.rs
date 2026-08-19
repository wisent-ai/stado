//! Read-only stdio JSON-RPC MCP server for stado.
//!
//! Implements newline-delimited JSON-RPC 2.0 on stdin, one response line per
//! request on stdout, and diagnostics on stderr only.
//! Every tool is dispatched by
//! shelling out to the stado CLI in a subprocess, so the CLI stays the
//! single source of truth. Only read-only, non-spending subcommands are
//! exposed; money-spending and mutating verbs are absent by design.
//!
//! CLI resolution is intentionally binary-only:
//! `$STADO_BIN` (explicit override, used by tests and custom installs) ->
//! a `stado` binary in the same directory as the running `stado-mcp`
//! executable -> plain `stado` (resolved on PATH at spawn time; a spawn
//! failure surfaces as a "stado CLI not found" tool error).
//!
//! Error codes match the Python implementation exactly: -32700 parse
//! error, -32601 method/tool not found, and -32000 for internal errors
//! (Python's `CODE_INTERNAL_ERROR` — note this is NOT the JSON-RPC
//! spec's -32603; the Python value is ported as written).

use std::io::{BufRead, Write};
use std::sync::LazyLock;
use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::procutil::{run_capture, Capture};

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
    fn internal(message: impl Into<String>) -> Self {
        Self {
            code: CODE_INTERNAL_ERROR,
            message: message.into(),
        }
    }
}

/// Positional/flag argument spec for one tool (Python `_REGISTRY[].arg`).
struct ArgSpec {
    name: &'static str,
    required: bool,
    desc: &'static str,
    flag: Option<&'static str>,
}

/// Read-only allow-list entry (Python `_REGISTRY[]`).
struct ToolSpec {
    name: &'static str,
    cli: &'static [&'static str],
    desc: &'static str,
    arg: Option<ArgSpec>,
}

/// The 16 read-only tools, in Python `_REGISTRY` order.
const REGISTRY: &[ToolSpec] = &[
    ToolSpec {
        name: "stado_status",
        cli: &["status"],
        desc: "List queued/running/completed/failed GPU jobs as a table (read-only).",
        arg: Some(ArgSpec {
            name: "filter",
            required: false,
            desc: "Optional job-id or batch-id substring to narrow the listing.",
            flag: None,
        }),
    },
    ToolSpec {
        name: "stado_cost_report",
        cli: &["cost", "report"],
        desc: "Per-target/per-model dollar spend from completed jobs (read-only).",
        arg: None,
    },
    ToolSpec {
        name: "stado_quota_show",
        cli: &["quota", "show", "--json"],
        desc: "GPU quota totals across the configured providers, as JSON (read-only).",
        arg: None,
    },
    ToolSpec {
        name: "stado_quota_catalog",
        cli: &["quota", "catalog", "--json"],
        desc: "Full GPU catalog for each configured provider, as JSON (read-only).",
        arg: None,
    },
    ToolSpec {
        name: "stado_quota_requests",
        cli: &["quota", "requests", "--json"],
        desc: "In-flight quota-increase requests and support comms, as JSON (read-only).",
        arg: None,
    },
    ToolSpec {
        name: "stado_profiles",
        cli: &["profiles"],
        desc: "List submit profiles, or print one profile's resolved JSON (read-only).",
        arg: Some(ArgSpec {
            name: "name",
            required: false,
            desc: "Optional profile name; omit to list every profile.",
            flag: None,
        }),
    },
    ToolSpec {
        name: "stado_schedule_list",
        cli: &["schedule", "list"],
        desc: "List all recurring (cron) job schedules (read-only).",
        arg: None,
    },
    ToolSpec {
        name: "stado_schedule_show",
        cli: &["schedule", "show"],
        desc: "Print a single schedule's full JSON by id (read-only).",
        arg: Some(ArgSpec {
            name: "schedule_id",
            required: true,
            desc: "The schedule id to display.",
            flag: None,
        }),
    },
    ToolSpec {
        name: "stado_registry_pull",
        cli: &["registry", "pull"],
        desc: "Print the GCS-hosted compute-target registry as JSON (read-only).",
        arg: None,
    },
    ToolSpec {
        name: "stado_host_health",
        cli: &["host", "health", "--json"],
        desc: "Return a registry-managed host's latest health beacon, log tail, and immutable object metadata as JSON (read-only).",
        arg: Some(ArgSpec {
            name: "target",
            required: true,
            desc: "Registry target name or declared hostname.",
            flag: None,
        }),
    },
    ToolSpec {
        name: "stado_artifact_list",
        cli: &["artifact", "list", "--json"],
        desc: "List immutable artifact versions and metadata as JSON (read-only).",
        arg: Some(ArgSpec {
            name: "type",
            required: false,
            desc: "Optional artifact type filter.",
            flag: Some("--type"),
        }),
    },
    ToolSpec {
        name: "stado_artifact_show",
        cli: &["artifact", "show", "--json"],
        desc: "Resolve and return one artifact manifest as JSON (read-only).",
        arg: Some(ArgSpec {
            name: "ref",
            required: true,
            desc: "Artifact version or alias reference.",
            flag: None,
        }),
    },
    ToolSpec {
        name: "stado_artifact_resolve",
        cli: &["artifact", "resolve", "--json"],
        desc: "Resolve an artifact alias to an immutable version (read-only).",
        arg: Some(ArgSpec {
            name: "ref",
            required: true,
            desc: "Artifact version or alias reference.",
            flag: None,
        }),
    },
    ToolSpec {
        name: "stado_artifact_verify",
        cli: &["artifact", "verify", "--json"],
        desc: "Re-run generic and type-specific artifact verification (read-only).",
        arg: Some(ArgSpec {
            name: "ref",
            required: true,
            desc: "Artifact version or alias reference.",
            flag: None,
        }),
    },
    ToolSpec {
        name: "stado_artifact_lineage",
        cli: &["artifact", "lineage", "--json"],
        desc: "Return artifact producer, dependencies, and aliases (read-only).",
        arg: Some(ArgSpec {
            name: "ref",
            required: true,
            desc: "Artifact version or alias reference.",
            flag: None,
        }),
    },
    ToolSpec {
        name: "stado_vast_status",
        cli: &["vast", "status"],
        desc: "Show Vast.ai's current view of our machine (rentals, listed); read-only.",
        arg: None,
    },
];

/// Python `_tool_schema`.
fn tool_schema(arg: Option<&ArgSpec>) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();
    if let Some(arg) = arg {
        properties.insert(
            arg.name.to_string(),
            json!({"type": "string", "description": arg.desc}),
        );
        if arg.required {
            required.push(Value::from(arg.name));
        }
    }
    json!({"type": "object", "properties": Value::Object(properties), "required": Value::Array(required)})
}

/// Python `tool_definitions()`: name/description/inputSchema per tool.
pub fn tool_definitions() -> Vec<Value> {
    REGISTRY
        .iter()
        .map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.desc,
                "inputSchema": tool_schema(tool.arg.as_ref()),
            })
        })
        .collect()
}

/// Python `TOOLS` (module-level, built once).
static TOOLS: LazyLock<Vec<Value>> = LazyLock::new(tool_definitions);

fn tool_by_name(name: &str) -> Option<&'static ToolSpec> {
    REGISTRY.iter().find(|tool| tool.name == name)
}

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

/// Python `_error`.
fn error_response(rid: &Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": JSONRPC_VERSION,
        "id": rid.clone(),
        "error": {"code": code, "message": message},
    })
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

/// Serialize one response frame. Python uses `json.dumps` defaults
/// (", " / ": " separators); keep them for byte parity.
fn frame(message: &Value) -> String {
    crate::queue::python_json_dumps(message).expect("JSON serialization is infallible")
}

/// Run the stdin loop until EOF (Python `serve`). Owns `writer`
/// exclusively (frames only); diagnostics go to stderr via `tracing`.
pub fn serve<R: BufRead, W: Write>(reader: R, writer: &mut W) {
    for raw in reader.lines() {
        let Ok(raw) = raw else { break };
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let request: Value = match serde_json::from_str(line) {
            Ok(request) => request,
            Err(_) => {
                let _ = writeln!(
                    writer,
                    "{}",
                    frame(&error_response(
                        &Value::Null,
                        CODE_PARSE_ERROR,
                        "parse error"
                    ))
                );
                let _ = writer.flush();
                continue;
            }
        };
        if !request.is_object() {
            let _ = writeln!(
                writer,
                "{}",
                frame(&error_response(
                    &Value::Null,
                    CODE_PARSE_ERROR,
                    "request must be a JSON object"
                ))
            );
            let _ = writer.flush();
            continue;
        }
        if let Some(response) = handle(&request) {
            let _ = writeln!(writer, "{}", frame(&response));
            let _ = writer.flush();
        }
    }
}
