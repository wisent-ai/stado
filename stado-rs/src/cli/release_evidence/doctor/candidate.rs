//! The candidate's own liveness: the one remote program this command owns,
//! and the section of the report it answers.

use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::deploy::{host_channel, production_runner, shlex_quote};
use crate::release_agent::HostReleaseState;

use super::super::constants::{
    HEALTH_NO_CANDIDATE, HEALTH_OK, HEALTH_UNPROBED, HEALTH_UNREACHABLE,
};

/// The marker the candidate probe prints, in the tab-delimited `STADO_*`
/// family every script on this channel speaks.
const CANDIDATE_MARKER: &str = "STADO_CANDIDATE";

/// The one remote program this module owns: is the recorded candidate pid
/// still there, and what does its readiness path answer.
///
/// `kill -0` is the shell builtin, not `/bin/kill`, so the probe forks
/// nothing to establish liveness — the fact the incident's state file
/// reported (`pid 46748 is gone`) and the fact it did not (whether the port
/// answers) come back from one round trip.
///
/// A curl failure is not an error here. "Nothing is listening" is an answer
/// to the operator's question, so the status word is empty and this side
/// reports [`HEALTH_UNREACHABLE`]; a script that exited non-zero would turn
/// the diagnosis into a transport failure.
const CANDIDATE_PROBE_BODY: &str = r#"set -u
LC_ALL=C
export LC_ALL
if kill -0 "$pid" 2>/dev/null; then
  alive=true
else
  alive=false
fi
status=$(/usr/bin/curl --silent --output /dev/null --max-time 3 \
  --write-out '%{http_code}' "http://127.0.0.1:$port$readiness_path" 2>/dev/null) || status=""
printf 'STADO_CANDIDATE\t%s\t%s\n' "$alive" "$status"
"#;

/// The probe bound to one recorded candidate.
///
/// `pid` and `port` are typed integers, so they reach the shell as digits.
/// The readiness path is the registry's own declaration and is quoted the
/// way [`crate::deploy::host_inventory::remote_inventory_script`] quotes its
/// declared program set — the same rule, so no operator input and no
/// unquoted registry value ever reaches the remote shell.
fn candidate_probe_script(pid: i32, port: u16, readiness_path: &str) -> String {
    format!(
        "pid={pid}\nport={port}\nreadiness_path={}\n{CANDIDATE_PROBE_BODY}",
        shlex_quote(readiness_path)
    )
}

/// The candidate section: the port, whether the recorded pid is still there,
/// and what the readiness path answered.
///
/// Every field stays present with a `null` when there is nothing to probe,
/// because the desktop client reads a fixed shape and an absent key would
/// read as a missing candidate exactly where a dead one is the finding.
pub(super) async fn candidate_section(
    target: &crate::targets::ComputeTarget,
    state: Option<&HostReleaseState>,
    readiness_path: Option<&str>,
) -> Result<Value, CmdError> {
    let Some(candidate) = state.and_then(|state| state.candidate.as_ref()) else {
        return Ok(json!({
            "port": Value::Null,
            "health_status": HEALTH_NO_CANDIDATE,
            "pid_alive": Value::Null,
        }));
    };
    let Some(readiness_path) = readiness_path else {
        return Ok(json!({
            "port": candidate.port,
            "health_status": HEALTH_UNPROBED,
            "pid_alive": Value::Null,
        }));
    };
    let script = candidate_probe_script(candidate.pid, candidate.port, readiness_path);
    let output = host_channel::run_script(target, &script, &production_runner())
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let mut pid_alive = Value::Null;
    let mut health = HEALTH_UNREACHABLE.to_string();
    for line in output.stdout.lines() {
        let fields = host_channel::marker_fields(line);
        if fields.first() != Some(&CANDIDATE_MARKER) || fields.len() != 3 {
            continue;
        }
        pid_alive = Value::from(fields[1] == "true");
        health = match fields[2].parse::<u16>() {
            Ok(status) if (200..300).contains(&status) => HEALTH_OK.to_string(),
            // curl writes `000` when it never got a response, which is the
            // same finding as an empty status: nothing answered.
            Ok(0) => HEALTH_UNREACHABLE.to_string(),
            Ok(status) => format!("http_{status}"),
            Err(_) => HEALTH_UNREACHABLE.to_string(),
        };
    }
    Ok(json!({
        "port": candidate.port,
        "health_status": health,
        "pid_alive": pid_alive,
    }))
}
