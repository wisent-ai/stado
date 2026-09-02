//! `stado service watch-spawn` — sit on one host and name the parent of the
//! next process that matches a program, while that parent is still alive.
//!
//! NO Python original. This exists because of a diagnosis that no shipped
//! command could finish. On charless-mac-mini an **undeclared**
//! `stado agent --target charless-mac-mini` kept coming back within one to
//! four minutes of being reaped. Every replacement was read with `ppid 1` and
//! no launchd label holding it, which says only one thing: whatever started it
//! had already exited, so the process reparented to launchd. The question
//! "what started it" was therefore unanswerable from any snapshot taken after
//! the fact, and every snapshot this fleet can take is after the fact.
//!
//! Why the existing readers cannot do it:
//!
//! - [`super::host_exec`] is an exact allowlist of argument-free read-only
//!   programs. Its `ps ax -o pid -o ppid -o etime -o comm` entry deliberately
//!   carries no `command`, because process arguments are where the secrets
//!   are, and every stado unit executes the same binary — so `comm` cannot
//!   tell an agent from a resolver. Widening that table is the wrong repair
//!   and its own module says so.
//! - [`super::service::reap_undeclared_processes`] does read full argv, but
//!   one invocation is one snapshot, and it is a signalling command besides.
//! - Driving either from here in a loop cannot sample faster than an SSH
//!   round trip, which on this fleet is tens of seconds. A parent that
//!   backgrounds a child and exits lives for a fraction of one.
//!
//! So the loop has to run ON the host, and that is the whole design:
//!
//! - one fixed remote program, no interpolation except a vetted command
//!   substring, a sample count and a sleep, exactly the contract
//!   [`super::service::REAP_SCRIPT`] holds;
//! - it **signals nothing, starts nothing and writes nothing**. It reads
//!   `ps` on an interval and prints. A watch that could also act would be a
//!   supervisor, and this fleet already has too many of those;
//! - the ancestry of a new arrival is resolved out of the SAME `ps` snapshot
//!   that first saw it, not by asking the host again. Asking again is how the
//!   answer gets lost: by the time a second `ps` runs the parent is gone and
//!   the child reads `ppid 1`, which is the state that made the question
//!   unanswerable in the first place. Each ancestor also carries a live
//!   `alive` re-check, so a report can say whether the parent was still
//!   running at the moment its child was caught.
//!
//! The match never enters any argv. It is handed to `awk` through the
//! environment, because a `ps` sweep looking for `stado agent` would otherwise
//! find the `awk` that is looking for it and report the searcher as the
//! arrival. That is not a hypothetical: it is the first thing this script did.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::service::quote_command_match;
use super::{host_channel, DeployError, Runner};
use crate::targets::ComputeTarget;

/// Longest watch a single invocation will hold the channel open for.
///
/// An hour is far past any respawn cadence worth catching, and a bound means
/// a forgotten watch cannot pin an SSH session open forever.
pub const MAX_SECONDS: u64 = 3600;

/// Shortest gap between samples, in milliseconds.
///
/// Below this the sampler spends more time forking `ps` than waiting, and on a
/// six-hundred-process host that is a measurable load on a machine somebody
/// else is using.
pub const MIN_INTERVAL_MS: u64 = 200;

/// Longest gap between samples. Past this the watch is not catching a parent,
/// it is taking snapshots, and [`super::service`] already does that.
pub const MAX_INTERVAL_MS: u64 = 10_000;

/// Slack added to the watch window for connection setup and teardown, so the
/// channel's own bound never fires before the remote loop has said `DONE`.
const TIMEOUT_SLACK: Duration = Duration::from_secs(60);

/// Read-only: `ps` on an interval, and nothing else. It starts nothing, stops
/// nothing, signals nothing, writes no file and needs no sudo.
///
/// `@MATCH@` is vetted by [`quote_command_match`] — the same charset the
/// reaper's own filter allows, widened by exactly a space.
const WATCH_SCRIPT: &str = "set -u
if [ \"$(/usr/bin/uname -s)\" != Darwin ]; then
  printf 'STADO_WATCH_UNSUPPORTED\\t%s\\n' \"$(/usr/bin/uname -s)\"
  exit 0
fi
match=@MATCH@
seconds=@SECONDS@
gap=@GAP@
self=$$
# This shell and every process above it are excluded by pid. The script itself
# arrives on stdin so the match is not in anybody's argv, but the sweep must
# still never report its own reader as an arrival.
mine=''
walk=$self
while [ -n \"$walk\" ] && [ \"$walk\" != 0 ] && [ \"$walk\" != 1 ]; do
  mine=\"$mine $walk\"
  walk=$(/bin/ps -p \"$walk\" -o ppid= 2>/dev/null | /usr/bin/tr -d ' ')
done
started=$(/bin/date +%s)
deadline=$(( started + seconds ))
known=''
first=yes
seq=0
samples=0
while :; do
  # ONE snapshot per sample. Every fact about this round -- who is new, who
  # its parent is, what that parent runs -- is read out of this one string,
  # because a second `ps` is a second moment and the parent may not be in it.
  snapshot=$(/bin/ps ax -o pid= -o ppid= -o lstart= -o command= 2>/dev/null)
  samples=$(( samples + 1 ))
  # The match rides the ENVIRONMENT, never argv: an `awk` invoked with
  # `stado agent` on its command line is itself a line containing
  # `stado agent`, and the sweep reported the searcher every single sample.
  hits=$(printf '%s\\n' \"$snapshot\" \
    | STADO_WATCH_MATCH=\"$match\" /usr/bin/awk 'index($0, ENVIRON[\"STADO_WATCH_MATCH\"]) > 0 { print $1 }')
  for pid in $hits; do
    case \" $mine \" in *\" $pid \"*) continue ;; esac
    case \" $known \" in *\" $pid \"*) continue ;; esac
    known=\"$known $pid\"
    row=$(printf '%s\\n' \"$snapshot\" \
      | /usr/bin/awk -v want=\"$pid\" '$1 == want { print; exit }' | /usr/bin/tr '\\t\\r\\n' ' ')
    if [ \"$first\" = yes ]; then
      printf 'STADO_WATCH_BASELINE\\t%s\\t%s\\n' \"$pid\" \"$row\"
      continue
    fi
    seq=$(( seq + 1 ))
    printf 'STADO_WATCH_ARRIVAL\\t%s\\t%s\\t%s\\t%s\\n' \\
      \"$seq\" \"$pid\" \"$(( $(/bin/date +%s) - started ))\" \"$row\"
    # The ancestry, out of the same snapshot, deepest-first from the arrival.
    # `alive` is asked of the live process table right now, so a report can
    # distinguish a parent that is still running from one already gone.
    up=$pid
    depth=0
    while [ -n \"$up\" ] && [ \"$up\" != 0 ] && [ \"$depth\" -lt 16 ]; do
      line=$(printf '%s\\n' \"$snapshot\" \
        | /usr/bin/awk -v want=\"$up\" '$1 == want { print; exit }' | /usr/bin/tr '\\t\\r\\n' ' ')
      if [ -z \"$line\" ]; then break; fi
      if /bin/ps -p \"$up\" -o pid= >/dev/null 2>&1; then alive=yes; else alive=no; fi
      printf 'STADO_WATCH_ANCESTOR\\t%s\\t%s\\t%s\\t%s\\t%s\\n' \"$seq\" \"$depth\" \"$up\" \"$alive\" \"$line\"
      if [ \"$up\" = 1 ]; then break; fi
      up=$(printf '%s' \"$line\" | /usr/bin/awk '{ print $2 }')
      depth=$(( depth + 1 ))
    done
  done
  first=no
  if [ \"$(/bin/date +%s)\" -ge \"$deadline\" ]; then break; fi
  /bin/sleep \"$gap\"
done
printf 'STADO_WATCH_DONE\\t%s\\t%s\\n' \"$samples\" \"$(( $(/bin/date +%s) - started ))\"
";

/// One `ps` row, split the way `ps ax -o pid= -o ppid= -o lstart= -o command=`
/// prints it.
///
/// `lstart` is five whitespace-separated tokens (`Tue Sep  1 16:20:32 2026`)
/// and the command is everything after them, so the split is positional and
/// the command is never truncated at its first space.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessRow {
    pub pid: String,
    pub ppid: String,
    pub started_at: String,
    pub command: String,
}

impl ProcessRow {
    /// Parse one row, or `None` when it is short enough that a field would
    /// have to be invented.
    pub fn parse(row: &str) -> Option<Self> {
        let fields: Vec<&str> = row.split_whitespace().collect();
        if fields.len() < 8 {
            return None;
        }
        Some(Self {
            pid: fields[0].to_string(),
            ppid: fields[1].to_string(),
            started_at: fields[2..7].join(" "),
            command: fields[7..].join(" "),
        })
    }

    pub fn to_json(&self) -> Value {
        json!({
            "pid": self.pid,
            "ppid": self.ppid,
            "started_at": self.started_at,
            "command": self.command,
        })
    }
}

/// One process that matched and was already running when the watch opened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Baseline {
    pub row: ProcessRow,
}

/// One process that matched and was NOT running when the watch opened, with
/// the ancestry read from the snapshot that first saw it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Arrival {
    pub sequence: u32,
    /// Seconds after the watch opened.
    pub after_seconds: u64,
    pub row: ProcessRow,
    /// Index 0 is the arrival itself; index 1 is its parent, and so on to
    /// pid 1 or to the first ancestor the snapshot no longer holds.
    pub ancestry: Vec<Ancestor>,
}

impl Arrival {
    /// The parent, when the snapshot still held one. `None` means the arrival
    /// was already reparented — the state that makes this question hard.
    pub fn parent(&self) -> Option<&Ancestor> {
        self.ancestry.get(1)
    }

    pub fn to_json(&self) -> Value {
        json!({
            "sequence": self.sequence,
            "after_seconds": self.after_seconds,
            "process": self.row.to_json(),
            "ancestry": self.ancestry.iter().map(Ancestor::to_json).collect::<Vec<_>>(),
        })
    }
}

/// One step up the chain from an arrival.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ancestor {
    pub depth: u32,
    /// Was this process still in the live table when the arrival was caught?
    /// A parent reported `false` had already exited, which is exactly how the
    /// child came to read `ppid 1`.
    pub alive: bool,
    pub row: ProcessRow,
}

impl Ancestor {
    pub fn to_json(&self) -> Value {
        json!({
            "depth": self.depth,
            "alive": self.alive,
            "pid": self.row.pid,
            "ppid": self.row.ppid,
            "started_at": self.row.started_at,
            "command": self.row.command,
        })
    }
}

/// Everything one watch saw.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchReport {
    pub host: String,
    pub matched: String,
    pub seconds: u64,
    pub interval_ms: u64,
    pub samples: u32,
    pub elapsed_seconds: u64,
    pub baseline: Vec<Baseline>,
    pub arrivals: Vec<Arrival>,
    /// Set when the host is not Darwin, naming the system it reported.
    pub unsupported: Option<String>,
}

/// Render the sleep argument. BSD `sleep` takes a decimal, and a whole number
/// is spelled without a fraction so the common case reads as `1`.
fn gap_argument(interval_ms: u64) -> String {
    if interval_ms.is_multiple_of(1000) {
        (interval_ms / 1000).to_string()
    } else {
        format!("{}.{:03}", interval_ms / 1000, interval_ms % 1000)
    }
}

/// Watch one host for processes matching `command_match`, for `seconds`.
///
/// Signals nothing. The only thing this can do to a host is read `ps`.
pub async fn watch_spawns(
    target: &ComputeTarget,
    command_match: &str,
    seconds: u64,
    interval_ms: u64,
    runner: &Runner,
) -> Result<WatchReport, DeployError> {
    let matched = quote_command_match(command_match)?;
    if seconds == 0 || seconds > MAX_SECONDS {
        return Err(DeployError(format!(
            "watch length must be between 1 and {MAX_SECONDS} seconds; {seconds} is outside it"
        )));
    }
    if !(MIN_INTERVAL_MS..=MAX_INTERVAL_MS).contains(&interval_ms) {
        return Err(DeployError(format!(
            "sample interval must be between {MIN_INTERVAL_MS} and {MAX_INTERVAL_MS} ms; \
             {interval_ms} is outside it"
        )));
    }
    let script = WATCH_SCRIPT
        .replace("@MATCH@", &format!("\"{matched}\""))
        .replace("@SECONDS@", &seconds.to_string())
        .replace("@GAP@", &gap_argument(interval_ms));
    let bound = Duration::from_secs(seconds) + TIMEOUT_SLACK;
    let output = host_channel::run_script_with_timeout(target, &script, bound, runner).await?;
    if !output.ok() {
        return Err(DeployError(host_channel::last_error_line(
            &output,
            "the spawn watch did not complete",
        )));
    }
    Ok(parse_watch(
        &target.name,
        &matched,
        seconds,
        interval_ms,
        &output.stdout,
    ))
}

/// Turn the marker stream into a report. Pure — covered by unit tests.
pub fn parse_watch(
    host: &str,
    matched: &str,
    seconds: u64,
    interval_ms: u64,
    stdout: &str,
) -> WatchReport {
    let mut report = WatchReport {
        host: host.to_string(),
        matched: matched.to_string(),
        seconds,
        interval_ms,
        ..WatchReport::default()
    };
    for line in stdout.lines() {
        match host_channel::marker_fields(line).as_slice() {
            ["STADO_WATCH_UNSUPPORTED", system] => {
                report.unsupported = Some((*system).trim().to_string());
            }
            ["STADO_WATCH_BASELINE", _pid, row] => {
                if let Some(row) = ProcessRow::parse(row) {
                    report.baseline.push(Baseline { row });
                }
            }
            ["STADO_WATCH_ARRIVAL", sequence, _pid, after, row] => {
                if let Some(row) = ProcessRow::parse(row) {
                    report.arrivals.push(Arrival {
                        sequence: sequence.trim().parse().unwrap_or_default(),
                        after_seconds: after.trim().parse().unwrap_or_default(),
                        row,
                        ancestry: Vec::new(),
                    });
                }
            }
            ["STADO_WATCH_ANCESTOR", sequence, depth, _pid, alive, row] => {
                let sequence: u32 = sequence.trim().parse().unwrap_or_default();
                let Some(row) = ProcessRow::parse(row) else {
                    continue;
                };
                let ancestor = Ancestor {
                    depth: depth.trim().parse().unwrap_or_default(),
                    alive: alive.trim() == "yes",
                    row,
                };
                if let Some(arrival) = report
                    .arrivals
                    .iter_mut()
                    .find(|arrival| arrival.sequence == sequence)
                {
                    arrival.ancestry.push(ancestor);
                }
            }
            ["STADO_WATCH_DONE", samples, elapsed] => {
                report.samples = samples.trim().parse().unwrap_or_default();
                report.elapsed_seconds = elapsed.trim().parse().unwrap_or_default();
            }
            _ => {}
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gap_renders_whole_and_fractional_seconds() {
        assert_eq!(gap_argument(1000), "1");
        assert_eq!(gap_argument(2000), "2");
        assert_eq!(gap_argument(500), "0.500");
        assert_eq!(gap_argument(250), "0.250");
    }

    #[test]
    fn row_keeps_the_whole_command_after_the_five_lstart_tokens() {
        let row = ProcessRow::parse(
            "38348 1 Tue Sep  1 16:35:37 2026 /u/.stado/bin/stado agent --target mini",
        )
        .expect("row parses");
        assert_eq!(row.pid, "38348");
        assert_eq!(row.ppid, "1");
        assert_eq!(row.started_at, "Tue Sep 1 16:35:37 2026");
        assert_eq!(row.command, "/u/.stado/bin/stado agent --target mini");
    }

    #[test]
    fn a_row_missing_fields_is_dropped_rather_than_invented() {
        assert!(ProcessRow::parse("38348 1 Tue Sep").is_none());
    }

    #[test]
    fn an_arrival_carries_the_parent_the_snapshot_still_held() {
        let stdout = "STADO_WATCH_BASELINE\t3963\t3963 1 Tue Sep  1 16:20:32 2026 /b/stado agent --target mini\n\
             STADO_WATCH_ARRIVAL\t1\t40111\t63\t40111 40109 Tue Sep  1 17:59:01 2026 /b/stado agent --target mini\n\
             STADO_WATCH_ANCESTOR\t1\t0\t40111\tyes\t40111 40109 Tue Sep  1 17:59:01 2026 /b/stado agent --target mini\n\
             STADO_WATCH_ANCESTOR\t1\t1\t40109\tyes\t40109 348 Tue Sep  1 17:59:01 2026 /bin/bash /u/keepalive.sh\n\
             STADO_WATCH_ANCESTOR\t1\t2\t348\tyes\t348 1 Wed Aug 26 21:45:35 2026 /bin/bash /u/supervise.sh\n\
             STADO_WATCH_DONE\t64\t63\n";
        let report = parse_watch("mini", "stado agent", 300, 1000, stdout);
        assert_eq!(report.baseline.len(), 1);
        assert_eq!(report.samples, 64);
        assert_eq!(report.arrivals.len(), 1);
        let arrival = &report.arrivals[0];
        assert_eq!(arrival.after_seconds, 63);
        assert_eq!(arrival.row.ppid, "40109");
        let parent = arrival.parent().expect("the parent was still alive");
        assert!(parent.alive);
        assert_eq!(parent.row.command, "/bin/bash /u/keepalive.sh");
        assert_eq!(arrival.ancestry.len(), 3);
    }

    #[test]
    fn an_already_reparented_arrival_reports_no_parent() {
        let stdout = "STADO_WATCH_ARRIVAL\t1\t40111\t9\t40111 1 Tue Sep  1 17:59:01 2026 /b/stado agent --target mini\n\
             STADO_WATCH_ANCESTOR\t1\t0\t40111\tyes\t40111 1 Tue Sep  1 17:59:01 2026 /b/stado agent --target mini\n\
             STADO_WATCH_ANCESTOR\t1\t1\t1\tyes\t1 0 Wed Aug 26 21:45:29 2026 /sbin/launchd\n\
             STADO_WATCH_DONE\t10\t9\n";
        let report = parse_watch("mini", "stado agent", 300, 1000, stdout);
        let arrival = &report.arrivals[0];
        assert_eq!(arrival.row.ppid, "1");
        assert_eq!(
            arrival.parent().map(|parent| parent.row.command.as_str()),
            Some("/sbin/launchd")
        );
    }

    #[test]
    fn a_non_darwin_host_says_so_instead_of_reporting_nothing() {
        let report = parse_watch(
            "box",
            "stado agent",
            60,
            1000,
            "STADO_WATCH_UNSUPPORTED\tLinux\n",
        );
        assert_eq!(report.unsupported.as_deref(), Some("Linux"));
        assert!(report.arrivals.is_empty());
    }

    #[test]
    fn the_match_never_reaches_the_remote_argv() {
        // The searcher must not be findable by its own search: `awk` reads the
        // pattern from the environment, so no process command line carries it.
        assert!(WATCH_SCRIPT.contains("ENVIRON[\\\"STADO_WATCH_MATCH\\\"]"));
        assert!(!WATCH_SCRIPT.contains("awk -v m="));
    }
}
