//! `stado host disk TARGET` — current disk usage of a registry host
//! alongside the state of the registry cleanup policy that governs it.
//!
//! NO Python original: item four of `docs/missing-commands.md`. Shape and
//! rules come from [`crate::deploy::host_reboot`] via
//! [`crate::deploy::host_channel`].
//!
//! Three parts, deliberately reported together. "97% full" on its own does
//! not tell an operator whether anything is going to be done about it, and
//! "the janitor last ran at 04:12" on its own does not say whether it
//! helped. The July incident was precisely the pair coming apart: a box at
//! zero free bytes whose cleanup policy looked fine in the registry.
//!
//! No part invents a schema.
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
//! - Local APFS snapshots come from `tmutil listlocalsnapshots /`, and they
//!   are here because NOTHING in this product can reclaim them and their
//!   blocks are already inside the `used` figure above. On
//!   `control-host` on 2026-08-18 the janitor's cleaners and the three
//!   `host reclaim` filesystem stages between them accounted for every
//!   consumer an operator could act on, and three OS-update snapshots sat
//!   outside all of it — the kind of thing that holds tens of GiB and turns
//!   "the product says the disk is accounted for" into a false statement.
//!   Reported, never touched. macOS publishes no size for a snapshot:
//!   `tmutil`, `diskutil apfs listSnapshots` and `diskutil info` all name
//!   them and none of them measures them (checked on macOS 26.5 on both this
//!   control plane's host and the mini), so the count and the host's own
//!   names are reported and no byte figure is invented from them.
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

/// Substitution point for the janitor's lock path in [`REMOTE_SCRIPT`], on
/// the same terms: a crate constant, shell-quoted before it is spliced.
const LOCK_PATH_MARK: &str = "@LOCK_PATH@";

/// The fixed remote program, with the janitor's state path spliced in by
/// [`remote_script`]. Read-only: disk usage, cleanup state, snapshots, and a
/// bounded two-level inventory of the two writable roots that dominate macOS.
const REMOTE_SCRIPT_TEMPLATE: &str = r#"set -u
/bin/df -Pk / 2>/dev/null | while IFS= read -r row; do
  set -- $row
  case "${1:-}" in
    Filesystem|"") continue ;;
  esac
  printf 'STADO_DISK\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "${1:-}" "${2:-}" "${3:-}" "${4:-}" "${5:-}" "${6:-}"
done
state="$HOME/@STATE_PATH@"
if [ -r "$state" ]; then
  printf 'STADO_CLEANUP_STATE\t%s\n' "$(/usr/bin/tr -d '\t\r\n' < "$state")"
else
  printf 'STADO_CLEANUP_STATE_MISSING\t%s\n' "$state"
fi
lock="$HOME/@LOCK_PATH@"
# Who holds the janitor's run lock. `lock_busy` in a cleanup report and
# `cleanup_in_progress` in an agent's capacity broadcast are the same fact
# seen from two sides, and neither one names the holder -- so a host can
# report both for hours, scan nothing, and refuse to admit work, with no
# command able to say which process to look at. On charless-mac-mini that
# cost most of a day. `lsof` is the only reader that answers it; the path is
# fixed by the product, never supplied by an operator.
if [ -e "$lock" ] && [ -x /usr/sbin/lsof ]; then
  /usr/sbin/lsof -Fpc -- "$lock" 2>/dev/null | {
    holder_pid=''
    while IFS= read -r field; do
      case "$field" in
        p*) holder_pid=${field#p} ;;
        c*)
          if [ -n "$holder_pid" ]; then
            printf 'STADO_CLEANUP_LOCK\t%s\t%s\n' "$holder_pid" "${field#c}"
            holder_pid=''
          fi
          ;;
      esac
    done
  }
  printf 'STADO_CLEANUP_LOCK_END\t%s\n' "$lock"
fi
if [ -x /usr/bin/tmutil ]; then
  /usr/bin/tmutil listlocalsnapshots / 2>/dev/null | while IFS= read -r row; do
    case "$row" in
      com.apple.*) printf 'STADO_SNAPSHOT\t%s\n' "$row" ;;
    esac
  done
  printf 'STADO_SNAPSHOT_END\t%s\n' 'listed'
fi
if [ "$(/usr/bin/uname 2>/dev/null || /bin/uname)" = "Darwin" ]; then
  for spec in "$HOME:2" "/private/var:2" "/private/var/folders:5" "$HOME/.local/share:4" "$HOME/.local/state:4" "$HOME/Library/Caches:3" "$HOME/.cargo/git:3" "$HOME/.stado/local-storage:4" "$HOME/.stado/local-backup:4"; do
    root=${spec%:*}
    depth=${spec##*:}
    [ -d "$root" ] || continue
    /usr/bin/du -xk -d "$depth" "$root" 2>/dev/null |
      /usr/bin/sort -nr |
      /usr/bin/head -n 40 |
      while IFS='	' read -r blocks path; do
        [ -n "$blocks" ] && [ -n "$path" ] || continue
        printf 'STADO_DISK_ITEM\t%s\t%s\n' "$blocks" "$path"
      done
  done
for clone_root in /private/var/folders/*/*/X/org.chromium.Chromium.code_sign_clone; do
  [ -d "$clone_root" ] || continue
  total=$(/usr/bin/find "$clone_root" -maxdepth 1 -type d -name 'code_sign_clone.*' 2>/dev/null | /usr/bin/wc -l | /usr/bin/tr -d ' ')
  day_old=$(/usr/bin/find "$clone_root" -maxdepth 1 -type d -name 'code_sign_clone.*' -mtime +0 2>/dev/null | /usr/bin/wc -l | /usr/bin/tr -d ' ')
  hour_old=$(/usr/bin/find "$clone_root" -maxdepth 1 -type d -name 'code_sign_clone.*' -mmin +60 2>/dev/null | /usr/bin/wc -l | /usr/bin/tr -d ' ')
  printf 'STADO_CLONE_SUMMARY\t%s\t%s\t%s\t%s\n' "$clone_root" "$total" "$hour_old" "$day_old"
done
fi
"#;

/// The remote program with the janitor's state and lock paths in place.
pub fn remote_script() -> String {
    REMOTE_SCRIPT_TEMPLATE
        .replace(
            STATE_PATH_MARK,
            &shlex_quote(&disk_cleanup::state_relative_path()),
        )
        .replace(
            LOCK_PATH_MARK,
            &shlex_quote(&disk_cleanup::lock_relative_path()),
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

/// `df -Pk` 1024-byte blocks as GiB, one decimal.
///
/// This module owns the unit because it owns the `df` invocation, and both
/// [`crate::deploy::host_gates`] and [`crate::deploy::host_reclaim`] report
/// free space in GiB against a registry policy that declares its watermarks in
/// GiB (`low_free_gb * `[`disk_cleanup::GIB`]). Three spellings of the same
/// division would eventually be three different numbers on one host.
pub fn gib_from_blocks(blocks: f64) -> f64 {
    (blocks / (1024.0 * 1024.0) * 10.0).round() / 10.0
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
    /// Which process wrote the report this reading came from, and the version
    /// of the binary that wrote it.
    ///
    /// The state file has several writers on an always-on host: the queue agent
    /// every tick, and a `disk-cleanup --watch` unit on its own timer. On
    /// 2026-08-31 the agent reported `interval_noop` with no errors at
    /// 14:55:24Z and this command read `invalid_or_unavailable_policy` from the
    /// same path 46 seconds later. Both readings were true about their own
    /// writer and neither was true about the host, so `outcome` alone told an
    /// operator whichever answer arrived last.
    ///
    /// Reporting it does not arbitrate. It makes the reading say whose verdict
    /// it is, which is the difference between a fact and a coin toss.
    pub writer: Option<String>,
    pub writer_version: Option<String>,
    pub free_bytes_before: Option<i64>,
    pub free_bytes_after: Option<i64>,
    /// `free_bytes_after - free_bytes_before` of the recorded pass. Free
    /// space can fall during a pass while other processes write, so this
    /// is signed and reported as measured rather than clamped.
    pub freed_bytes: Option<i64>,
    pub next_pass_at: Option<String>,
    /// The low watermark the janitor VALIDATED on its last pass, in bytes, or
    /// `None` when the recorded report does not identify a canonical policy.
    ///
    /// Read through the janitor's own
    /// [`disk_cleanup::validated_report_low_bytes`], which is the same
    /// function the queue agent resolves `disk_low_bytes` with. It matters
    /// here because it, and not the registry declaration, is the number
    /// admission is gated on when a host cannot read the registry — the Mac
    /// mini published `disk_pressure_unresolved` for hours and the CLI could
    /// not show what threshold that verdict was measured against.
    pub low_bytes: Option<i64>,
    /// The state document was there but did not parse.
    pub error: Option<String>,
}

/// The local APFS snapshots this host is holding, which nothing in this
/// product removes.
///
/// Their blocks are inside `df`'s used figure, so free space does not come
/// back until they are thinned — and `stado host reclaim` cannot thin them:
/// dropping a snapshot is dropping a restore point, which is an operator's
/// decision about that machine's recovery and not a janitor's about its disk.
/// Reported so nobody reads a reclamation that freed nothing and concludes the
/// space is unexplained.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalSnapshots {
    /// Whether the host could be asked at all. False on every Linux host and
    /// on any Mac without `tmutil`: "nobody looked" is not "there are none".
    pub supported: bool,
    /// The snapshot names as the host listed them, verbatim — the same
    /// strings `tmutil deletelocalsnapshots` and `tmutil thinlocalsnapshots`
    /// take, so what is printed here is what an operator can act on.
    ///
    /// No sizes: macOS publishes none for a snapshot (see the module header),
    /// and a byte figure derived from anything else here would be a guess
    /// wearing a number's clothes.
    pub names: Vec<String>,
}

/// One measured directory in the bounded host inventory.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiskItem {
    pub blocks_kb: i64,
    pub path: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CloneSummary {
    pub path: String,
    pub total: i64,
    pub older_than_hour: i64,
    pub older_than_day: i64,
}

/// One process holding the janitor's run lock, as the host's `lsof` named it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LockHolder {
    pub pid: String,
    pub command: String,
}

/// Everything one host answered.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DiskReading {
    pub usage: Option<DiskUsage>,
    pub clone_summaries: Vec<CloneSummary>,
    pub state: CleanupState,
    pub snapshots: LocalSnapshots,
    pub inventory: Vec<DiskItem>,
    /// Who holds the run lock right now. Empty with `lock_read` true means
    /// nothing holds it, which is a different fact from never having looked.
    pub lock_holders: Vec<LockHolder>,
    pub lock_read: bool,
    pub lock_path: Option<String>,
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
                reading.state = parse_state(payload, policy_interval_seconds);
            }
            ["STADO_CLEANUP_STATE_MISSING", path] => {
                reading.state = CleanupState {
                    path: Some((*path).to_string()),
                    ..CleanupState::default()
                };
            }
            ["STADO_CLEANUP_LOCK", pid, command] => {
                reading.lock_holders.push(LockHolder {
                    pid: (*pid).trim().to_string(),
                    command: (*command).trim().to_string(),
                });
            }
            // Printed whether or not anything held it, so "nobody is holding
            // the lock" is distinguishable from "this host could not be asked".
            ["STADO_CLEANUP_LOCK_END", path] => {
                reading.lock_read = true;
                reading.lock_path = Some((*path).to_string());
            }
            ["STADO_SNAPSHOT", name] => {
                reading.snapshots.supported = true;
                reading.snapshots.names.push((*name).to_string());
            }
            // The host has `tmutil` and listed what it has, which is how a Mac
            // with no snapshots at all is told apart from a host nobody could
            // ask.
            ["STADO_SNAPSHOT_END", _] => reading.snapshots.supported = true,
            ["STADO_DISK_ITEM", blocks, path] => {
                if let Ok(blocks_kb) = blocks.parse::<i64>() {
                    reading.inventory.push(DiskItem {
                        blocks_kb,
                        path: (*path).to_string(),
                    });
                }
            }
            ["STADO_CLONE_SUMMARY", path, total, hour, day] => {
                if let (Ok(total), Ok(older_than_hour), Ok(older_than_day)) = (
                    total.parse::<i64>(),
                    hour.parse::<i64>(),
                    day.parse::<i64>(),
                ) {
                    reading.clone_summaries.push(CloneSummary {
                        path: (*path).to_string(),
                        total,
                        older_than_hour,
                        older_than_day,
                    });
                }
            }
            _ => {}
        }
    }
    reading
}

/// Epoch seconds as the ISO-8601 spelling the rest of the fleet uses.
fn iso_from_epoch(epoch: f64) -> Option<String> {
    DateTime::from_timestamp(epoch.trunc() as i64, u32::default()).map(crate::models::isoformat_utc)
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
        writer: text("writer"),
        writer_version: text("writer_version"),
        free_bytes_before: free_before,
        free_bytes_after: free_after,
        freed_bytes: match (free_before, free_after) {
            (Some(before), Some(after)) => Some(after - before),
            _ => None,
        },
        next_pass_at,
        low_bytes: report.and_then(disk_cleanup::validated_report_low_bytes),
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
            "writer": state.writer,
            "writer_version": state.writer_version,
            "free_bytes_before": state.free_bytes_before,
            "free_bytes_after": state.free_bytes_after,
            "freed_bytes": state.freed_bytes,
            "next_pass_at": state.next_pass_at,
            "low_bytes": state.low_bytes,
            "error": state.error,
        }),
    );
    // The other half of every `lock_busy` and `cleanup_in_progress` an
    // operator has ever read: which process is holding the run lock.
    report.insert(
        "cleanup_lock".to_string(),
        json!({
            "read": reading.lock_read,
            "path": reading.lock_path,
            "held": !reading.lock_holders.is_empty(),
            "holders": reading
                .lock_holders
                .iter()
                .map(|holder| json!({"pid": holder.pid, "command": holder.command}))
                .collect::<Vec<Value>>(),
        }),
    );
    // Reported next to the usage it does not appear in: `size_bytes` is
    // deliberately absent rather than null, because macOS states no size and a
    // key an operator could read as "zero" is worse than a key that is not
    // there. `reclaimable_by_stado` is the finding.
    let snapshots = &reading.snapshots;
    report.insert(
        "local_snapshots".to_string(),
        json!({
            "supported": snapshots.supported,
            "count": snapshots.names.len(),
            "names": snapshots.names,
            "reclaimable_by_stado": snapshots.names.iter().any(|name| {
                name.starts_with("com.apple.TimeMachine.") && name.ends_with(".local")
            }),
        }),
    );
    report.insert(
        "inventory".to_string(),
        Value::Array(
            reading
                .inventory
                .iter()
                .map(|item| {
                    json!({
                        "path": item.path,
                        "size_gb": gib_from_blocks(item.blocks_kb as f64),
                    })
                })
                .collect(),
        ),
    );
    report.insert(
        "chromium_clones".to_string(),
        Value::Array(
            reading
                .clone_summaries
                .iter()
                .map(|summary| {
                    json!({
                        "path": summary.path,
                        "total": summary.total,
                        "older_than_hour": summary.older_than_hour,
                        "older_than_day": summary.older_than_day,
                    })
                })
                .collect(),
        ),
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
