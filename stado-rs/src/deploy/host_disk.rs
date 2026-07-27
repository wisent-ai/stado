//! `stado host disk TARGET` — current disk usage of a registry host
//! alongside the state of the registry cleanup policy that governs it.
//!
//! NO Python original: item four of `docs/missing-commands.md`. Shape and
//! rules come from [`crate::deploy::host_reboot`] via
//! [`crate::deploy::host_channel`].
//!
//! Two halves, deliberately reported together. "97% full" on its own does
//! not tell an operator whether anything is going to be done about it, and
//! "the janitor last ran at 04:12" on its own does not say whether it
//! helped. The July incident was precisely the pair coming apart: a box at
//! zero free bytes whose cleanup policy looked fine in the registry.
//!
//! Neither half invents a schema.
//!
//! - Usage comes from `df -Pk /` — the POSIX output format, so the columns
//!   are the same on macOS and Linux, unlike the default macOS layout,
//!   which inserts three inode columns before the mount point.
//! - Policy comes from the registry's own
//!   [`crate::targets::DiskCleanupPolicy`], serialized as it stands.
//! - State comes from the janitor's own state file, named by
//!   [`crate::providers::local::disk_cleanup::state_relative_path`] and
//!   written by that module's `write_state`. The `last pass`, `freed
//!   bytes` and `next scheduled pass` this command reports are all derived
//!   from that document; nothing here re-implements the janitor's
//!   bookkeeping.
//!
//! Like [`crate::deploy::host_recovery`]'s script, the remote program is
//! written as an escaped string: `\\t` / `\\n` are the literal backslash
//! sequences the remote `printf` expands.

use chrono::{DateTime, TimeDelta};
use serde_json::{json, Map, Value};

use super::host_channel;
use super::{shlex_quote, DeployError, Runner};
use crate::providers::local::disk_cleanup;
use crate::targets::ComputeTarget;

/// `status` for a clean read.
pub const OK_STATUS: &str = "ok";

/// Substitution point for the janitor's state path in [`REMOTE_SCRIPT`].
/// The value is a crate constant, never registry or operator data, and it
/// is shell-quoted before it is spliced.
const STATE_PATH_MARK: &str = "@STATE_PATH@";

/// The fixed remote program, with the janitor's state path spliced in by
/// [`remote_script`]. Read-only: one `df` and one `cat`.
const REMOTE_SCRIPT_TEMPLATE: &str = "set -u
/bin/df -Pk / 2>/dev/null | while IFS= read -r row; do
  set -- $row
  case \"${1:-}\" in
    Filesystem|\"\") continue ;;
  esac
  printf 'STADO_DISK\\t%s\\t%s\\t%s\\t%s\\t%s\\t%s\\n' \\
    \"${1:-}\" \"${2:-}\" \"${3:-}\" \"${4:-}\" \"${5:-}\" \"${6:-}\"
done
state=\"$HOME/@STATE_PATH@\"
if [ -r \"$state\" ]; then
  printf 'STADO_CLEANUP_STATE\\t%s\\n' \"$(/usr/bin/tr -d '\\t\\r\\n' < \"$state\")\"
else
  printf 'STADO_CLEANUP_STATE_MISSING\\t%s\\n' \"$state\"
fi
";

/// The remote program with the janitor's state path in place.
pub fn remote_script() -> String {
    REMOTE_SCRIPT_TEMPLATE.replace(
        STATE_PATH_MARK,
        &shlex_quote(&disk_cleanup::state_relative_path()),
    )
}

/// One `df -Pk` row, in the units the host reported (1024-byte blocks for
/// the three sizes, a percentage string for capacity).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiskUsage {
    pub filesystem: String,
    pub blocks_kb: String,
    pub used_kb: String,
    pub available_kb: String,
    pub capacity: String,
    pub mounted_on: String,
}

/// What the host's janitor state file says about the last and next pass.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CleanupState {
    /// Absent when the host has no state file at all — a host whose
    /// janitor has never completed a pass, which is itself the finding.
    pub present: bool,
    /// Where the state file was looked for, when it was not there.
    pub path: Option<String>,
    pub last_pass_at: Option<String>,
    pub last_success_at: Option<String>,
    pub outcome: Option<String>,
    pub free_bytes_before: Option<i64>,
    pub free_bytes_after: Option<i64>,
    /// `free_bytes_after - free_bytes_before` of the recorded pass. Free
    /// space can fall during a pass while other processes write, so this
    /// is signed and reported as measured rather than clamped.
    pub freed_bytes: Option<i64>,
    pub next_pass_at: Option<String>,
    /// The state document was there but did not parse.
    pub error: Option<String>,
}

/// Everything one host answered.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DiskReading {
    pub usage: Option<DiskUsage>,
    pub state: CleanupState,
}

/// Fold the marker lines of stdout into a reading.
pub fn parse_output(stdout: &str, policy_interval_seconds: Option<i64>) -> DiskReading {
    let mut reading = DiskReading::default();
    for line in stdout.lines() {
        match host_channel::marker_fields(line).as_slice() {
            ["STADO_DISK", filesystem, blocks, used, available, capacity, mounted] => {
                reading.usage = Some(DiskUsage {
                    filesystem: (*filesystem).to_string(),
                    blocks_kb: (*blocks).to_string(),
                    used_kb: (*used).to_string(),
                    available_kb: (*available).to_string(),
                    capacity: (*capacity).to_string(),
                    mounted_on: (*mounted).to_string(),
                });
            }
            ["STADO_CLEANUP_STATE", payload] => {
                reading.state = parse_state(*payload, policy_interval_seconds);
            }
            ["STADO_CLEANUP_STATE_MISSING", path] => {
                reading.state = CleanupState {
                    path: Some((*path).to_string()),
                    ..CleanupState::default()
                };
            }
            _ => {}
        }
    }
    reading
}

/// Epoch seconds as the ISO-8601 spelling the rest of the fleet uses.
fn iso_from_epoch(epoch: f64) -> Option<String> {
    DateTime::from_timestamp(epoch.trunc() as i64, u32::default())
        .map(crate::models::isoformat_utc)
}

/// Read the janitor's state document.
///
/// The keys are exactly the ones
/// [`crate::providers::local::disk_cleanup`]'s `write_state` emits:
/// `last_attempt_at` at the top level and the whole previous report under
/// `report`. `next_pass_at` is the interval gate in `run_with_lock` read
/// forwards — that gate compares `now - last_attempt_at` against the
/// registry policy's `check_interval_seconds`, so the next pass is the
/// sum of the two.
pub fn parse_state(payload: &str, policy_interval_seconds: Option<i64>) -> CleanupState {
    let document: Value = match serde_json::from_str(payload) {
        Ok(value) => value,
        Err(exc) => {
            return CleanupState {
                present: true,
                error: Some(exc.to_string()),
                ..CleanupState::default()
            };
        }
    };
    let report = document.get("report");
    let field = |key: &str| report.and_then(|value| value.get(key));
    let text = |key: &str| field(key).and_then(Value::as_str).map(str::to_string);
    let free_before = field("free_bytes_before").and_then(Value::as_i64);
    let free_after = field("free_bytes_after").and_then(Value::as_i64);
    let last_attempt = document.get("last_attempt_at").and_then(Value::as_f64);
    let next_pass_at = match (last_attempt, policy_interval_seconds) {
        (Some(attempt), Some(interval)) => {
            DateTime::from_timestamp(attempt.trunc() as i64, u32::default())
                .and_then(|stamp| stamp.checked_add_signed(TimeDelta::seconds(interval)))
                .map(crate::models::isoformat_utc)
        }
        _ => None,
    };
    CleanupState {
        present: true,
        path: None,
        last_pass_at: text("started_at").or_else(|| last_attempt.and_then(iso_from_epoch)),
        last_success_at: text("last_success_at"),
        outcome: text("outcome"),
        free_bytes_before: free_before,
        free_bytes_after: free_after,
        freed_bytes: match (free_before, free_after) {
            (Some(before), Some(after)) => Some(after - before),
            _ => None,
        },
        next_pass_at,
        error: None,
    }
}

/// The reading as the `--json` report, in `host reboot`'s report shape.
pub fn to_report(target: &ComputeTarget, reading: &DiskReading) -> Map<String, Value> {
    let mut report = host_channel::base_report(target);
    report.insert(
        "usage".to_string(),
        reading.usage.as_ref().map_or(Value::Null, |usage| {
            json!({
                "filesystem": usage.filesystem,
                "blocks_kb": usage.blocks_kb,
                "used_kb": usage.used_kb,
                "available_kb": usage.available_kb,
                "capacity": usage.capacity,
                "mounted_on": usage.mounted_on,
            })
        }),
    );
    // The registry policy verbatim — same struct the janitor resolves, so
    // the operator is reading the declaration the host actually obeys.
    report.insert(
        "policy".to_string(),
        target
            .disk_cleanup
            .as_ref()
            .and_then(|policy| serde_json::to_value(policy).ok())
            .unwrap_or(Value::Null),
    );
    let state = &reading.state;
    report.insert(
        "cleanup_state".to_string(),
        json!({
            "present": state.present,
            "path": state.path,
            "last_pass_at": state.last_pass_at,
            "last_success_at": state.last_success_at,
            "outcome": state.outcome,
            "free_bytes_before": state.free_bytes_before,
            "free_bytes_after": state.free_bytes_after,
            "freed_bytes": state.freed_bytes,
            "next_pass_at": state.next_pass_at,
            "error": state.error,
        }),
    );
    report
}

/// Read disk usage and cleanup state from one canonical registry host.
pub async fn disk_host(target_name: &str, runner: &Runner) -> Result<Value, DeployError> {
    let target = host_channel::canonical_target(target_name).await?;
    let output = host_channel::run_script(&target, &remote_script(), runner).await?;
    let interval = target
        .disk_cleanup
        .as_ref()
        .map(|policy| policy.check_interval_seconds);
    let reading = parse_output(&output.stdout, interval);
    let mut report = to_report(&target, &reading);
    host_channel::finish_report(&mut report, &output, OK_STATUS, "ssh failed");
    Ok(Value::Object(report))
}
