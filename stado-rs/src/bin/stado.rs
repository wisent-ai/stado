//! `stado` CLI — Rust port of `stado/cli.py`'s click entry point.
//!
//! Behaves identically regardless of argv[0] (the `wc` alias is just a copy
//! of this binary). Exit codes: 0 on success, 2 for usage errors (clap parse
//! failures and not-yet-implemented commands) and 1 for runtime errors, both
//! matching click; plus 69 (`sysexits.h` `EX_UNAVAILABLE`) when the failure
//! classified as one a retry can clear. The classification, the human
//! sentence and the structured log line all come from
//! [`stado::cli::main_entry`]; this binary only installs the log writer and
//! hands the code to the shell. See `docs/cli.md`.

use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    // Log filter from $RUST_LOG, defaulting to warn (Python logs to stderr
    // at WARNING by default).
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("warn,gcp_auth=error")),
        )
        .with_writer(std::io::stderr)
        .init();
    let code = stado::cli::main_entry().await;
    std::process::exit(code);
}
