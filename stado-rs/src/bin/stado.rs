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

/// Stack for the thread the whole CLI runs on.
///
/// `#[tokio::main]` blocks on the entry future on the process's main thread,
/// and macOS gives that thread 8 MiB that no `ulimit` can raise after exec. A
/// debug build's future for `main_entry` is one state machine holding every
/// awaited command's locals inlined, and on 2026-08-31 it crossed that
/// boundary: `./target/debug/stado service list`, `service show`,
/// `service env`, `service reap` and `doctor` all died with
/// `thread 'main' has overflowed its stack` before parsing finished, while the
/// release binary at the same commit ran them fine.
///
/// A verification instrument that aborts is worse than a slow one: every agent
/// working in this repository verifies through `./target/debug/stado`, and for
/// part of that day the answer to "is the fleet healthy" was a SIGSEGV that
/// looked like a host problem. The runtime is built on a thread this file
/// sizes, so the limit is a number in the product rather than whatever the
/// kernel hands the main thread.
const ENTRY_STACK_BYTES: usize = 64 * 1024 * 1024;

fn main() {
    // Log filter from $RUST_LOG, defaulting to warn (Python logs to stderr
    // at WARNING by default). Installed on the real main thread: the writer is
    // process-global and outlives the entry thread.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("warn,gcp_auth=error")),
        )
        .with_writer(std::io::stderr)
        .init();
    let entry = std::thread::Builder::new()
        .name("stado-cli".to_string())
        .stack_size(ENTRY_STACK_BYTES)
        .spawn(|| {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("tokio runtime")
                .block_on(stado::cli::main_entry())
        })
        .expect("spawn the cli thread");
    // A panic inside the entry has already printed through the panic hook, and
    // click's own code for an unhandled error is 1.
    let code = entry.join().unwrap_or(1);
    std::process::exit(code);
}
