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
//! Error codes match the Python implementation exactly:
//! -32700 parse
//! error, -32601 method/tool not found, and -32000 for internal errors
//! (Python's `CODE_INTERNAL_ERROR` — note this is NOT the JSON-RPC
//! spec's -32603; the Python value is ported as written).

mod dispatch;
mod protocol;
mod tools;
mod transport;

pub use dispatch::{call_tool, handle, stado_argv};
pub use protocol::{
    ToolError, CODE_INTERNAL_ERROR, CODE_METHOD_NOT_FOUND, CODE_PARSE_ERROR, JSONRPC_VERSION,
    PROTOCOL_VERSION, SUBPROCESS_TIMEOUT_SECONDS,
};
pub use tools::tool_definitions;
pub use transport::serve;
