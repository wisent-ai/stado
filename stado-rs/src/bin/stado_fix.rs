//! `stado-fix` — Rust port of `stado/failure_fixer/cli.py` (click group).
//!
//! NOTE (stale docs, ported faithfully): the Python CLI's help still says
//! "HMAC-sign + POST to model-router" but the implementation execs the
//! local `claude` CLI — this port follows the implementation.
//!
//! Exit codes match click: 2 for usage errors (clap parse failures), 1
//! for runtime failures and a job_id with no failed/ blob, 0 on success.

use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn,gcp_auth=error")))
        .with_writer(std::io::stderr)
        .init();
    let code = stado::failure_fixer::cli_main().await;
    std::process::exit(code);
}
