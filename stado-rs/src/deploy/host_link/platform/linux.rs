//! Linux: the journal's suspend unit for sleep/resume, the kernel journal for
//! interface changes — both read from the journal's own fields
//! (`journalctl -o json`), not from the wording of its messages.
//!
//! systemd marks the job of every unit it starts: the entry that begins the
//! suspend unit's start job carries `JOB_TYPE=start` and no result, and the
//! entry that ends it carries `JOB_RESULT`; the host goes down at the first
//! and is back at the second. The kernel journal names the subsystem of the
//! device a message is about in `_KERNEL_SUBSYSTEM`, and `net` is a network
//! interface, so every message the kernel attributes to an interface in the
//! window is an interface change, whatever its driver writes.
//!
//! Each reader answers none when `journalctl` is not on this host, and the
//! error with its cause when it ran and its answer could not be read.

use std::num::NonZeroU64;

use chrono::{DateTime, FixedOffset};
use serde_json::Value;

use crate::deploy::host_link::probes::{probe, resolve_program, window_minutes};
use crate::deploy::host_link::timestamps::{detail_of, iso, newest_changes};
use crate::deploy::host_link::InterfaceChange;
use crate::deploy::Runner;

const SUSPEND_UNIT: &str = "systemd-suspend.service";
/// The journal's own field names (systemd.journal-fields).
const REALTIME_FIELD: &str = "__REALTIME_TIMESTAMP";
const MESSAGE_FIELD: &str = "MESSAGE";
const JOB_TYPE_FIELD: &str = "JOB_TYPE";
const JOB_RESULT_FIELD: &str = "JOB_RESULT";
const START_JOB: &str = "start";
const KERNEL_SUBSYSTEM_FIELD: &str = "_KERNEL_SUBSYSTEM";
const NETWORK_SUBSYSTEM: &str = "net";

/// Every journal entry of `journalctl -o json`. A line that is not JSON is
/// the error: a journal with that entry silently missing is not a reading.
fn entries(stdout: &str) -> Result<Vec<Value>, String> {
    stdout
        .lines()
        .map(|line| {
            serde_json::from_str::<Value>(line).map_err(|error| {
                format!("journalctl wrote a line that is not JSON ({error}): {line}")
            })
        })
        .collect()
}

fn field<'a>(entry: &'a Value, name: &str) -> Option<&'a str> {
    entry.get(name).and_then(Value::as_str)
}

/// When the entry was written: the journal's microseconds since the epoch.
/// An entry that names none, or one that is no instant, is the error.
fn written_at(entry: &Value) -> Result<DateTime<FixedOffset>, String> {
    let raw = field(entry, REALTIME_FIELD)
        .ok_or_else(|| format!("a journal entry names no {REALTIME_FIELD}: {entry}"))?;
    let micros = raw.parse::<i64>().map_err(|error| {
        format!("{REALTIME_FIELD} {raw:?} is no count of microseconds: {error}")
    })?;
    DateTime::from_timestamp_micros(micros)
        .map(|stamp| stamp.fixed_offset())
        .ok_or_else(|| format!("{REALTIME_FIELD} {micros} is no instant"))
}

/// `(last_sleep_at, last_wake_at)` from the suspend unit's jobs.
///
/// A registry Linux host is typically a server that never suspends, so both
/// halves being null is the normal, honest answer there.
pub(in crate::deploy::host_link) async fn linux_sleep_wake(
    runner: &Runner,
) -> Result<Option<(Option<String>, Option<String>)>, String> {
    let Some(program) = resolve_program("journalctl")? else {
        return Ok(None);
    };
    let output = probe(
        runner,
        vec![
            program,
            "--no-pager".to_string(),
            // No "-- No entries --" or other informational line: every line
            // of the answer is an entry, so a line that is not JSON is a fault.
            "--quiet".to_string(),
            "--reverse".to_string(),
            "-o".to_string(),
            "json".to_string(),
            "-u".to_string(),
            SUSPEND_UNIT.to_string(),
        ],
    )
    .await?;

    let mut sleep = None;
    let mut wake = None;
    for entry in entries(&output.stdout)? {
        let stamp = written_at(&entry)?;
        let starts = field(&entry, JOB_TYPE_FIELD) == Some(START_JOB);
        let finished = field(&entry, JOB_RESULT_FIELD).is_some();
        if sleep.is_none() && starts && !finished {
            sleep = Some(iso(stamp));
        } else if wake.is_none() && finished {
            wake = Some(iso(stamp));
        }
        if sleep.is_some() && wake.is_some() {
            break;
        }
    }
    Ok(Some((sleep, wake)))
}

/// Interface changes inside the window from the kernel journal.
pub(in crate::deploy::host_link) async fn linux_interface_changes(
    runner: &Runner,
    window: NonZeroU64,
) -> Result<Option<Vec<InterfaceChange>>, String> {
    let Some(program) = resolve_program("journalctl")? else {
        return Ok(None);
    };
    let output = probe(
        runner,
        vec![
            program,
            "-k".to_string(),
            "--no-pager".to_string(),
            "--quiet".to_string(),
            "-o".to_string(),
            "json".to_string(),
            "--since".to_string(),
            format!("-{}min", window_minutes(window)),
        ],
    )
    .await?;

    let mut changes = Vec::new();
    for entry in entries(&output.stdout)? {
        if field(&entry, KERNEL_SUBSYSTEM_FIELD) != Some(NETWORK_SUBSYSTEM) {
            continue;
        }
        let stamp = written_at(&entry)?;
        let message = field(&entry, MESSAGE_FIELD)
            .ok_or_else(|| format!("a kernel network entry names no {MESSAGE_FIELD}: {entry}"))?;
        changes.push((stamp, detail_of(message)));
    }
    Ok(Some(newest_changes(changes)))
}
