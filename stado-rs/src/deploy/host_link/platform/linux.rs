//! Linux: the journal's suspend unit for sleep/resume, `journalctl -k` for
//! interface changes.

use crate::deploy::host_link::probes::{probe, resolve_program, window_minutes};
use crate::deploy::host_link::timestamps::{detail_of, iso, newest_changes, parse_stamp};
use crate::deploy::host_link::InterfaceChange;
use crate::deploy::Runner;

/// `(last_sleep_at, last_wake_at)` from the journal's suspend unit, or `None`
/// when journalctl is absent or failed.
///
/// A registry Linux host is typically a server that never suspends, so both
/// halves being null is the normal, honest answer there.
pub(in crate::deploy::host_link) async fn linux_sleep_wake(
    runner: &Runner,
) -> Option<(Option<String>, Option<String>)> {
    let program = resolve_program("journalctl")?;
    let output = probe(
        runner,
        vec![
            program,
            "--no-pager".to_string(),
            "-o".to_string(),
            "short-iso".to_string(),
            "-n".to_string(),
            "200".to_string(),
            "-u".to_string(),
            "systemd-suspend.service".to_string(),
        ],
    )
    .await?;

    let mut sleep = None;
    let mut wake = None;
    for line in output.stdout.lines().rev() {
        let Some(stamp) = line.split_whitespace().next().and_then(parse_stamp) else {
            continue;
        };
        // systemd's own wording for the unit that suspends the machine:
        // it starts as the host goes down and finishes as it comes back.
        if sleep.is_none() && line.contains("Starting") {
            sleep = Some(iso(stamp));
        } else if wake.is_none() && (line.contains("Finished") || line.contains("Stopped")) {
            wake = Some(iso(stamp));
        }
        if sleep.is_some() && wake.is_some() {
            break;
        }
    }
    Some((sleep, wake))
}

/// One journal line's own sentence: `journalctl -o short-iso` prefixes every
/// line with its timestamp and the hostname, and both are already answered by
/// the record around it (`at`, and the beacon's own host). What is left is
/// `kernel: ...`, which names the emitter and says what happened.
fn journal_message(line: &str) -> &str {
    line.splitn(3, ' ').nth(2).unwrap_or(line)
}

/// The kernel-log markers of a link transition, matched case-insensitively.
/// Copied from a live fleet host: `i40e ... eth0: NIC Link is Up, 1000 Mbps
/// Full Duplex` is the spelling a real interface change has there.
const LINUX_INTERFACE_MARKERS: [&str; 4] = [
    "link is up",
    "link is down",
    "link becomes ready",
    "carrier",
];

/// Interface changes inside the window from the kernel journal, or `None`
/// when journalctl is absent or failed.
pub(in crate::deploy::host_link) async fn linux_interface_changes(
    runner: &Runner,
    window: i64,
) -> Option<Vec<InterfaceChange>> {
    let program = resolve_program("journalctl")?;
    let output = probe(
        runner,
        vec![
            program,
            "-k".to_string(),
            "--no-pager".to_string(),
            "-o".to_string(),
            "short-iso".to_string(),
            "--since".to_string(),
            format!("-{}min", window_minutes(window)),
            "-n".to_string(),
            "500".to_string(),
        ],
    )
    .await?;

    let changes = output
        .stdout
        .lines()
        .filter_map(|line| {
            let lowered = line.to_lowercase();
            if !LINUX_INTERFACE_MARKERS
                .iter()
                .any(|marker| lowered.contains(marker))
            {
                return None;
            }
            let stamp = line.split_whitespace().next().and_then(parse_stamp)?;
            Some((stamp, detail_of(journal_message(line))))
        })
        .collect();
    Some(newest_changes(changes))
}
