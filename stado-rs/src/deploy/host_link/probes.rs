//! The probes: how wide a window one beacon reads, how one capped command
//! is run, and how a tool is found on `PATH`. Each says why it could not
//! answer, so the link block can carry the cause beside the null.

use std::num::NonZeroU64;

use nix::errno::Errno;
use nix::unistd::{access, AccessFlags};

use crate::deploy::{CommandOutput, CommandSpec, Runner};

/// Seconds in a minute, by the SI definition of the minute:
/// https://www.bipm.org/en/publications/si-brochure
const SECONDS_PER_MINUTE: u64 = 60;

/// How far back the interface-change window reaches: one beacon interval, so
/// consecutive beacons tile the timeline without this module persisting a
/// cursor of its own. The interval is the beacon unit's declared
/// `WC_HEALTH_INTERVAL_SECONDS`; without one there is no window, and the
/// interface changes are published as unread rather than over a guessed span.
/// A declared value that is not a positive whole number is the cause.
pub(super) fn window_seconds() -> Result<Option<NonZeroU64>, String> {
    let raw = match std::env::var("WC_HEALTH_INTERVAL_SECONDS") {
        Ok(raw) => raw,
        Err(std::env::VarError::NotPresent) => return Ok(None),
        Err(error) => {
            return Err(format!(
                "WC_HEALTH_INTERVAL_SECONDS cannot be read: {error}"
            ))
        }
    };
    raw.trim().parse::<NonZeroU64>().map(Some).map_err(|error| {
        format!("WC_HEALTH_INTERVAL_SECONDS is {raw:?}, not a positive whole number: {error}")
    })
}

/// The window in whole minutes, rounded up, which is the unit `log show
/// --last` and `journalctl --since` both take.
pub(super) fn window_minutes(window: NonZeroU64) -> u64 {
    window.get().div_ceil(SECONDS_PER_MINUTE)
}

/// Run one probe until the tool exits: its output, or why it did not answer
/// (it could not be started, or it exited unsuccessfully with its stderr).
pub(super) async fn probe(runner: &Runner, argv: Vec<String>) -> Result<CommandOutput, String> {
    let named = argv.join(" ");
    let output = runner(CommandSpec::new(argv))
        .await
        .map_err(|error| format!("`{named}` could not run: {error}"))?;
    if !output.ok() {
        return Err(format!(
            "`{named}` exited {}: {}",
            output.code,
            output.stderr.trim()
        ));
    }
    Ok(output)
}

/// The first file named `name` on `PATH` this process may execute; none when
/// no directory holds one. A candidate that cannot be inspected, or a unit
/// that declares no `PATH` at all, is the error with its cause.
///
/// A beacon runs under launchd or systemd with whatever `PATH` its unit
/// declares, and `tailscale` lives in a different directory on every macOS
/// install (`/usr/local/bin`, Homebrew, inside the app bundle). Resolving
/// here — rather than trying a list of absolute paths — keeps that answer in
/// the unit environment, which is the only place that knows it.
pub(super) fn resolve_program(name: &str) -> Result<Option<String>, String> {
    let Some(path) = std::env::var_os("PATH") else {
        return Err(format!(
            "the beacon unit declares no PATH, so `{name}` was not looked for"
        ));
    };
    for directory in std::env::split_paths(&path) {
        if directory.as_os_str().is_empty() {
            continue;
        }
        let candidate = directory.join(name);
        let metadata = match std::fs::metadata(&candidate) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "{} cannot be inspected: {error}",
                    candidate.display()
                ))
            }
        };
        if !metadata.is_file() {
            continue;
        }
        match access(&candidate, AccessFlags::X_OK) {
            Ok(()) => return Ok(Some(candidate.to_string_lossy().into_owned())),
            Err(Errno::EACCES) => continue,
            Err(error) => {
                return Err(format!(
                    "whether {} may be run cannot be read: {error}",
                    candidate.display()
                ))
            }
        }
    }
    Ok(None)
}
