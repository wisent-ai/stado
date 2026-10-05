//! Whether the unit answers afterwards, asked from the host itself and
//! ended by what the unit does, not by a count of attempts.

use crate::cli::web::deploy::{click, marker};
use crate::cli::CmdError;
use crate::deploy::{host_channel, Runner};
use crate::targets::ComputeTarget;

/// Probe the unit's readiness path from the host itself until the unit either
/// answers 200 or stops being the process that was started.
///
/// It has to run there. The declared port is loopback — the launcher binds
/// `127.0.0.1` deliberately, because the public entrance is the edge and not
/// the unit — so a probe from the operator's laptop reaches nothing, and a
/// probe from the tailnet address would be answering a different question.
///
/// The probe counts no attempts and cuts no request short. A unit that cannot
/// start ends: its process exits, or its supervisor starts it again, and
/// either is read from the supervisor's own record of the unit (launchd's
/// `pid`, `runs` and `last exit code`; systemd's `MainPID`, `NRestarts` and
/// `ExecMainStatus`) after every probe. A process that dies closes its
/// connection, which ends the request in flight, and a live process that is
/// still answering is the unit getting ready. Each probe follows the previous
/// one at once, so a 200 is seen as soon as the unit gives it.
///
/// The last state is carried out, not just the verdict. "Nothing answered on
/// the port" and "answered HTTP 503" are opposite findings with opposite
/// repairs: the first says the process is not listening, the second says it is
/// listening and telling you it is not ready.
const WEB_READY_BODY: &str = r#"
unready() {
  printf 'STADO_WEB_READY\tunready\t%s\n' "$1"
  exit 0
}
unit_state() {
  pid=''
  runs=''
  exited=''
  if [ "$(/usr/bin/uname -s)" = Darwin ]; then
    uid=$(/usr/bin/id -u)
    for domain in "gui/$uid" "user/$uid"; do
      show=$(/bin/launchctl print "$domain/$label" 2>/dev/null) || continue
      pid=$(printf '%s\n' "$show" | /usr/bin/awk -F' = ' '$1 ~ /^[[:space:]]*pid$/ { print $2; exit }')
      runs=$(printf '%s\n' "$show" | /usr/bin/awk -F' = ' '$1 ~ /^[[:space:]]*runs$/ { print $2; exit }')
      exited=$(printf '%s\n' "$show" | /usr/bin/awk -F' = ' '$1 ~ /^[[:space:]]*last exit code$/ { print $2; exit }')
      return 0
    done
    return 1
  fi
  show=$(/usr/bin/systemctl --user show -p MainPID -p NRestarts -p ExecMainStatus -p LoadState -- "$label.service" 2>/dev/null) || return 1
  [ "$(printf '%s\n' "$show" | /usr/bin/sed -n 's/^LoadState=//p')" = loaded ] || return 1
  pid=$(printf '%s\n' "$show" | /usr/bin/sed -n 's/^MainPID=//p')
  [ "$pid" = 0 ] && pid=''
  runs=$(printf '%s\n' "$show" | /usr/bin/sed -n 's/^NRestarts=//p')
  exited=$(printf '%s\n' "$show" | /usr/bin/sed -n 's/^ExecMainStatus=//p')
  return 0
}
unit_state || unready "$label is not loaded, so nothing was started to answer"
started_pid=$pid
started_runs=$runs
[ -n "$started_pid" ] || unready "$label is loaded but runs no process (last exit code ${exited:-unrecorded})"
attempt=0
while :; do
  attempt=$((attempt + 1))
  code="$(/usr/bin/curl -s -o /dev/null -w '%{http_code}' "$url" 2>/dev/null || true)"
  if [ "$code" = "200" ]; then
    printf 'STADO_WEB_READY\tready\tHTTP 200 after %s attempt(s) from pid %s\n' "$attempt" "$started_pid"
    exit 0
  fi
  if [ -z "$code" ] || [ "$code" = "000" ]; then
    last='nothing answered on the port'
  else
    last="answered HTTP $code"
  fi
  unit_state || unready "$label was unloaded after $attempt attempt(s); the last probe: $last"
  if [ "$pid" != "$started_pid" ] || [ "$runs" != "$started_runs" ]; then
    unready "pid $started_pid ended without answering 200 (last exit code ${exited:-unrecorded}; runs ${started_runs:-unrecorded} -> ${runs:-unrecorded}; now ${pid:-no process}) after $attempt attempt(s); the last probe: $last"
  fi
done
"#;

/// Ask the host whether the unit `label` answers on its own readiness path.
///
/// Returns `(verdict, detail)` rather than a bare boolean because the detail
/// is the whole value of the check: the caller turns an unready unit into a
/// refusal that names the port, the path, the process that ended and the last
/// thing the host saw.
pub(in crate::cli::web::deploy) async fn wait_until_ready(
    target: &ComputeTarget,
    label: &str,
    url: &str,
    runner: &Runner,
) -> Result<(String, String), CmdError> {
    let script = format!(
        "set -eu\nurl={}\nlabel={}\n{WEB_READY_BODY}",
        crate::deploy::shlex_quote(url),
        crate::deploy::shlex_quote(label),
    );
    let output = host_channel::run_script(target, &script, runner)
        .await
        .map_err(click)?;
    if !output.ok() {
        return Err(CmdError::click(format!(
            "{}: the readiness probe could not be run: {}",
            target.name,
            host_channel::last_error_line(&output, "the probe reported nothing")
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    let fields = marker(&output.stdout, "STADO_WEB_READY").ok_or_else(|| {
        CmdError::click(format!(
            "{}: the readiness probe returned no verdict, so whether {url} answers was never \
             established",
            target.name
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown)
    })?;
    let verdict = fields.first().copied().unwrap_or("unknown").to_string();
    let detail = fields
        .get(1)
        .copied()
        .filter(|detail| !detail.is_empty())
        .unwrap_or("the probe reported no detail")
        .to_string();
    Ok((verdict, detail))
}
