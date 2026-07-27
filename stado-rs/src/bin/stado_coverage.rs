//! `stado-coverage` — Rust port of `stado/coverage/cli.py` (click group).
//!
//! Exit codes match click: 2 for usage errors (clap parse failures and
//! UsageError equivalents like an unknown universe), 1 for runtime
//! failures and the empty-registry `list`, 0 on success.

use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn,gcp_auth=error")))
        .with_writer(std::io::stderr)
        .init();
    let code = stado::coverage::cli_main().await;
    std::process::exit(code);
}
