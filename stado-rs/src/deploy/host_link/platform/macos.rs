//! macOS: `pmset -g log` for sleep/wake, `log show` for interface changes.

use serde_json::Value;

use crate::deploy::host_link::probes::{probe, probe_within, resolve_program, window_minutes};
use crate::deploy::host_link::timestamps::{detail_of, iso, newest_changes, parse_stamp};
use crate::deploy::host_link::InterfaceChange;
use crate::deploy::Runner;

/// The `pmset -g log` transition kinds that end a sleep. A DarkWake is a
/// maintenance wake rather than a user one, but it is still the moment the
/// network came back, which is the question being asked.
const MACOS_WAKE_KINDS: [&str; 2] = ["Wake", "DarkWake"];

/// `(last_sleep_at, last_wake_at)` from `pmset -g log`, or `None` when pmset
/// is absent or failed.
///
/// The read is bounded from the end: the log holds tens of thousands of lines
/// and only the newest transition of each kind is wanted, so the scan walks
/// backwards and stops as soon as it has both.
pub(in crate::deploy::host_link) async fn macos_sleep_wake(
    runner: &Runner,
) -> Option<(Option<String>, Option<String>)> {
    let program = resolve_program("pmset")?;
    let output = probe_within(
        runner,
        vec![program, "-g".to_string(), "log".to_string()],
        crate::deploy::host_link::POWER_LOG_TIMEOUT,
    )
    .await?;

    let mut sleep = None;
    let mut wake = None;
    for line in output.stdout.lines().rev() {
        let mut fields = line.split_whitespace();
        let (Some(date), Some(time), Some(offset), Some(kind)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let is_sleep = kind == "Sleep";
        let is_wake = MACOS_WAKE_KINDS.contains(&kind);
        if (is_sleep && sleep.is_some()) || (is_wake && wake.is_some()) || (!is_sleep && !is_wake) {
            continue;
        }
        let Some(stamp) = parse_stamp(&format!("{date} {time} {offset}")) else {
            continue;
        };
        if is_sleep {
            sleep = Some(iso(stamp));
        } else {
            wake = Some(iso(stamp));
        }
        if sleep.is_some() && wake.is_some() {
            break;
        }
    }
    Some((sleep, wake))
}

/// The unified-log predicate for an interface change.
///
/// Anchored on `process == "configd"`, which is not decoration: an
/// unanchored `eventMessage CONTAINS` predicate over an eight hour window
/// took 2m52s on an M2 Max, while this one over three hours took 2.7s. The
/// process anchor is what lets the log store skip. `link` catches the link
/// state transitions and `SSID` the wifi association changes; the periodic
/// `publish success` heartbeat is deliberately not matched, because a
/// heartbeat is not a change.
const MACOS_INTERFACE_PREDICATE: &str =
    "process == \"configd\" AND (eventMessage CONTAINS \"link\" OR eventMessage CONTAINS \"SSID\")";

/// Interface changes inside the window from the unified log, or `None` when
/// `log` is absent or failed. `Some(vec![])` means the window was quiet.
pub(in crate::deploy::host_link) async fn macos_interface_changes(
    runner: &Runner,
    window: i64,
) -> Option<Vec<InterfaceChange>> {
    let program = resolve_program("log")?;
    let output = probe(
        runner,
        vec![
            program,
            "show".to_string(),
            "--last".to_string(),
            format!("{}m", window_minutes(window)),
            "--style".to_string(),
            "ndjson".to_string(),
            "--predicate".to_string(),
            MACOS_INTERFACE_PREDICATE.to_string(),
        ],
    )
    .await?;

    let changes = output
        .stdout
        .lines()
        .filter_map(|line| {
            let entry: Value = serde_json::from_str(line.trim()).ok()?;
            let stamp = parse_stamp(entry.get("timestamp")?.as_str()?)?;
            let message = entry.get("eventMessage")?.as_str()?;
            (!message.trim().is_empty()).then(|| (stamp, detail_of(message)))
        })
        .collect();
    Some(newest_changes(changes))
}
