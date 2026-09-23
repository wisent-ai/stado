//! `stado host uptime TARGET` — uptime, load averages and logged-in users
//! of one registry-managed host.
//!
//! NO Python original: item two of `docs/missing-commands.md`. Shape and
//! rules come from [`crate::deploy::host_reboot`] via
//! [`crate::deploy::host_channel`] — registry-authorized target, a FIXED
//! remote program, the shared ssh option set, the [`Runner`] seam, and a
//! report carrying `exit_code`, `status` and the last stderr line.
//!
//! The remote program does not just run `uptime` and hand back its line:
//! that line's shape differs between macOS ("load averages: a b c") and
//! Linux ("load average: a, b, c"), and screen-scraping it locally is how
//! a monitoring command starts lying. The script emits the tab-delimited
//! `STADO_*` markers of [`crate::deploy::host_recovery::parse_output`]
//! instead, reading the load averages from the kernel (`/proc/loadavg`, or
//! `vm.loadavg` via sysctl) and the sessions from `who`. The raw `uptime`
//! line is still reported verbatim, because an operator reading it wants
//! the box's own words.
//!
//! Like [`crate::deploy::host_recovery`]'s script, this one is written as
//! an escaped string rather than a raw string: `\\t` / `\\n` are the
//! literal backslash sequences the remote `printf` expands, not control
//! characters embedded in the Rust source.

use serde_json::{json, Map, Value};

use super::host_channel;
use super::{DeployError, Runner};
use crate::targets::ComputeTarget;

/// `status` for a clean read.
pub const OK_STATUS: &str = "ok";

/// The fixed remote program. Read-only throughout: it runs `uptime`, reads
/// the kernel load averages, lists login sessions and prints the short
/// hostname. Nothing is written, nothing is escalated, and no word of it
/// comes from the registry or from the operator.
pub const REMOTE_SCRIPT: &str = "set -u
uptime_line=$(/usr/bin/uptime 2>/dev/null | /usr/bin/tr -d '\\t\\r')
printf 'STADO_UPTIME\\t%s\\n' \"${uptime_line:-}\"
if [ -r /proc/loadavg ]; then
  load=$(/usr/bin/cut -d' ' -f1-3 /proc/loadavg 2>/dev/null)
else
  load=$(/usr/sbin/sysctl -n vm.loadavg 2>/dev/null | /usr/bin/tr -d '{}')
fi
set -- ${load:-}
printf 'STADO_LOAD\\t%s\\t%s\\t%s\\n' \"${1:-}\" \"${2:-}\" \"${3:-}\"
/usr/bin/who 2>/dev/null | while IFS= read -r session; do
  set -- $session
  printf 'STADO_USER\\t%s\\t%s\\t%s\\n' \"${1:-}\" \"${2:-}\" \"${3:-} ${4:-} ${5:-}\"
done
printf 'STADO_HOST\\t%s\\n' \"$(/bin/hostname -s 2>/dev/null)\"
";

/// One login session as `who` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub user: String,
    pub line: String,
    pub since: String,
}

/// The three kernel load averages, kept as the strings the host printed —
/// re-parsing them into floats would only add a rounding step between the
/// kernel and the operator.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoadAverage {
    pub one: String,
    pub five: String,
    pub fifteen: String,
}

/// One host's uptime reading.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UptimeReading {
    pub host: String,
    pub uptime: String,
    pub load: LoadAverage,
    pub sessions: Vec<Session>,
}

/// Fold the marker lines of stdout into a reading. An unknown or truncated
/// marker line is ignored rather than indexed into.
pub fn parse_output(stdout: &str) -> UptimeReading {
    let mut reading = UptimeReading::default();
    for line in stdout.lines() {
        match host_channel::marker_fields(line).as_slice() {
            ["STADO_UPTIME", text] => reading.uptime = (*text).trim().to_string(),
            ["STADO_LOAD", one, five, fifteen] => {
                reading.load = LoadAverage {
                    one: (*one).to_string(),
                    five: (*five).to_string(),
                    fifteen: (*fifteen).to_string(),
                };
            }
            ["STADO_USER", user, line_name, since] => reading.sessions.push(Session {
                user: (*user).to_string(),
                line: (*line_name).to_string(),
                since: (*since).trim().to_string(),
            }),
            ["STADO_HOST", host] => reading.host = (*host).to_string(),
            _ => {}
        }
    }
    reading
}

/// The reading as the `--json` report, in `host reboot`'s report shape.
pub fn to_report(target: &ComputeTarget, reading: &UptimeReading) -> Map<String, Value> {
    let mut report = host_channel::base_report(target);
    report.insert("host".to_string(), json!(reading.host));
    report.insert("uptime".to_string(), json!(reading.uptime));
    report.insert(
        "load_average".to_string(),
        json!({
            "one": reading.load.one,
            "five": reading.load.five,
            "fifteen": reading.load.fifteen,
        }),
    );
    report.insert(
        "users".to_string(),
        Value::Array(
            reading
                .sessions
                .iter()
                .map(|session| {
                    json!({"user": session.user, "line": session.line, "since": session.since})
                })
                .collect(),
        ),
    );
    report
}

/// Read uptime, load averages and login sessions from one canonical
/// registry host.
pub async fn uptime_host(target_name: &str, runner: &Runner) -> Result<Value, DeployError> {
    let target = host_channel::canonical_target(target_name).await?;
    let output = host_channel::run_script(&target, REMOTE_SCRIPT, runner).await?;
    let reading = parse_output(&output.stdout);
    let mut report = to_report(&target, &reading);
    host_channel::finish_report(&mut report, &output, OK_STATUS, "ssh failed");
    Ok(Value::Object(report))
}
