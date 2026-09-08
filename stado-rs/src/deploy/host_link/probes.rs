//! The probes: how wide a window one beacon reads, how one capped command
//! is run, and how a tool is found on `PATH`.

use super::{DEFAULT_WINDOW_SECONDS, MAX_WINDOW_SECONDS, MIN_WINDOW_SECONDS, PROBE_TIMEOUT};
use crate::deploy::{CommandOutput, CommandSpec, Runner};

/// How far back the interface-change window reaches: one beacon interval, so
/// consecutive beacons tile the timeline without this module persisting a
/// cursor of its own.
pub(super) fn window_seconds() -> i64 {
    std::env::var("WC_HEALTH_INTERVAL_SECONDS")
        .ok()
        .and_then(|raw| raw.trim().parse::<i64>().ok())
        .unwrap_or(DEFAULT_WINDOW_SECONDS)
        .clamp(MIN_WINDOW_SECONDS, MAX_WINDOW_SECONDS)
}

/// The window in whole minutes, which is the unit `log show --last` and
/// `journalctl --since` both take. Always at least one.
pub(super) fn window_minutes(window: i64) -> i64 {
    (window + 59) / 60
}

/// Run one probe. `None` covers every way a probe can fail to answer:
/// missing binary, spawn error, timeout, non-zero exit.
pub(super) async fn probe(runner: &Runner, argv: Vec<String>) -> Option<CommandOutput> {
    probe_within(runner, argv, PROBE_TIMEOUT).await
}

/// The same probe under the caller's own cap, for a read whose cost is the
/// size of a log rather than the reachability of a tool.
pub(super) async fn probe_within(
    runner: &Runner,
    argv: Vec<String>,
    cap: std::time::Duration,
) -> Option<CommandOutput> {
    let mut spec = CommandSpec::new(argv);
    spec.timeout = Some(cap);
    match runner(spec).await {
        Ok(output) if output.ok() => Some(output),
        _ => None,
    }
}

/// The first executable named `name` on `PATH`, or `None`.
///
/// A beacon runs under launchd or systemd with whatever `PATH` its unit
/// declares, and `tailscale` lives in a different directory on every macOS
/// install (`/usr/local/bin`, Homebrew, inside the app bundle). Resolving
/// here — rather than trying a list of absolute paths — keeps that answer in
/// the unit environment, which is the only place that knows it.
pub(super) fn resolve_program(name: &str) -> Option<String> {
    use std::os::unix::fs::PermissionsExt;

    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|directory| {
        if directory.as_os_str().is_empty() {
            return None;
        }
        let candidate = directory.join(name);
        let metadata = std::fs::metadata(&candidate).ok()?;
        (metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
            .then(|| candidate.to_string_lossy().into_owned())
    })
}
