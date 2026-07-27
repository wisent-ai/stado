//! `stado-watchdog` — Rust port of `stado/deploy/watchdog/cli.py`.
//!
//! Unlike the other stado entry points (click), the Python original uses
//! argparse; the port reproduces argparse's usage/error text and exit
//! codes (2 for argument errors, 0 for --help and a completed --once pass,
//! 1 when the upload fails and the local fallback is written).

use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn,gcp_auth=error")))
        .with_writer(std::io::stderr)
        .init();
    let code = stado::watchdog::cli_main().await;
    std::process::exit(code);
}
