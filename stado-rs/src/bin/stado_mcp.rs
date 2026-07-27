//! `stado-mcp` — Rust port of `stado/mcp/server.py`: read-only stdio
//! JSON-RPC MCP server (protocol 2024-11-05). Newline-delimited requests
//! on stdin, one response line per request on stdout, diagnostics on
//! stderr only.

use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    // Diagnostics must never touch stdout (the frame channel).
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("warn,gcp_auth=error")),
        )
        .with_writer(std::io::stderr)
        .init();
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    stado::mcp::serve(stdin.lock(), &mut stdout);
}
