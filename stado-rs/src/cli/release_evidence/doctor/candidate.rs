//! The candidate's own liveness: the one remote program this command owns,
//! and the section of the report it answers.

use std::net::{Ipv4Addr, SocketAddrV4};

use reqwest::StatusCode;
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
/// `kill -0` is the shell builtin, not `/bin/kill`, so checking the recorded
/// process does not fork another process. Liveness and readiness are observed
/// independently in one round trip; either can fail while the other succeeds.
///
/// A curl failure is not an error here. "Nothing is listening" is an answer
/// to the operator's question, so curl's non-zero exit empties the status
/// word and this side reports [`HEALTH_UNREACHABLE`]; a script that exited
/// non-zero would turn the diagnosis into a transport failure. The request
/// carries no time cut of its own: the candidate's answer or curl's own
/// failure ends it.
const CANDIDATE_PROBE_BODY: &str = r#"set -u
LC_ALL=C
export LC_ALL
if kill -0 "$pid" 2>/dev/null; then
  alive=true
else
  alive=false
fi
status=$(/usr/bin/curl --silent --output /dev/null \
  --write-out '%{http_code}' "http://$address$readiness_path" 2>/dev/null) || status=""
printf 'STADO_CANDIDATE\t%s\t%s\n' "$alive" "$status"
"#;

/// The probe bound to one recorded candidate.
///
/// `pid` and the address are typed values, so they reach the shell as digits
/// and dots. The candidate listens on the loopback interface at its recorded
/// port, as the release agent spawns it. The readiness path is the registry's
/// own declaration and is quoted the way
/// [`crate::deploy::host_inventory::remote_inventory_script`] quotes its
/// declared program set — the same rule, so no operator input and no
/// unquoted registry value ever reaches the remote shell.
fn candidate_probe_script(pid: i32, port: u16, readiness_path: &str) -> String {
    format!(
        "pid={pid}\naddress={}\nreadiness_path={}\n{CANDIDATE_PROBE_BODY}",
        SocketAddrV4::new(Ipv4Addr::LOCALHOST, port),
        shlex_quote(readiness_path)
    )
}

/// The health word for the status curl wrote: none (curl failed) is
/// unreachable, a success status is ok, any other status is named. A status
/// that is not an HTTP status is refused naming it.
fn health_of(status: &str) -> Result<String, CmdError> {
    if status.is_empty() {
        return Ok(HEALTH_UNREACHABLE.to_string());
    }
    match StatusCode::from_bytes(status.as_bytes()) {
        Ok(code) if code.is_success() => Ok(HEALTH_OK.to_string()),
        Ok(code) => Ok(format!("http_{}", code.as_u16())),
        Err(error) => Err(CmdError::refused(format!(
            "the candidate probe printed {status:?} as the readiness status, which is not an \
             HTTP status: {error}"
        ))),
    }
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
        .map_err(CmdError::from)?;
    // The probe prints exactly one marker line; any other line is the
    // channel's own output and is not read. A marker line of another shape,
    // or none at all, is refused naming what came back.
    let mut answer = None;
    for line in output.stdout.lines() {
        match host_channel::marker_fields(line).as_slice() {
            [marker, alive, status] if *marker == CANDIDATE_MARKER => {
                answer = Some((Value::from(*alive == "true"), health_of(status)?));
            }
            [marker, ..] if *marker == CANDIDATE_MARKER => {
                return Err(CmdError::refused(format!(
                    "the candidate probe printed a {CANDIDATE_MARKER} line that is not \
                     <alive>\\t<status>: {line:?}"
                )));
            }
            _ => {}
        }
    }
    let Some((pid_alive, health)) = answer else {
        return Err(CmdError::refused(format!(
            "the candidate probe printed no {CANDIDATE_MARKER} line; it answered: {:?}",
            output.stdout
        )));
    };
    Ok(json!({
        "port": candidate.port,
        "health_status": health,
        "pid_alive": pid_alive,
    }))
}
