//! `stado` CLI — Rust port of `stado/cli.py`'s click entry point.
//!
//! Behaves identically regardless of argv[0] (the `wc` alias is just a copy
//! of this binary). Exit codes match click: 2 for usage errors (clap parse
//! failures and not-yet-implemented commands), 1 for runtime errors, 0 on
//! success.

use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    // Log filter from $RUST_LOG, defaulting to warn (Python logs to stderr
    // at WARNING by default).
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn,gcp_auth=error")))
        .with_writer(std::io::stderr)
        .init();
    let code = stado::cli::main_entry().await;
    std::process::exit(code);
}
