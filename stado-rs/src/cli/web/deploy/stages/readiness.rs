//! Whether the unit answers afterwards, asked from the host itself and
//! bounded.

use std::time::Duration;

use crate::cli::web::deploy::{
    click, marker, READY_ATTEMPTS, READY_INTERVAL_SECONDS, READY_REQUEST_SECONDS,
};
use crate::cli::CmdError;
use crate::deploy::{host_channel, Runner};
use crate::targets::ComputeTarget;

/// Poll the unit's readiness path from the host itself.
///
/// It has to run there. The declared port is loopback — the launcher binds
/// `127.0.0.1` deliberately, because the public entrance is the edge and not
/// the unit — so a probe from the operator's laptop reaches nothing, and a
/// probe from the tailnet address would be answering a different question.
///
/// The loop is on the host rather than in this process for the same reason
/// the fetch is: one round trip instead of twenty, and the whole wait is
/// bounded by the script's own attempt count rather than by how long an ssh
/// connection happens to survive.
///
/// The last state is carried out, not just the verdict. "Nothing answered on
/// the port" and "answered HTTP 503" are opposite findings with opposite
/// repairs: the first says the process is not listening, the second says it is
/// listening and telling you it is not ready.
const WEB_READY_BODY: &str = r#"
attempt=0
last='no attempt was made'
while [ "$attempt" -lt "$attempts" ]; do
  attempt=$((attempt + 1))
  code="$(/usr/bin/curl -s -o /dev/null -m "$request_budget" -w '%{http_code}' "$url" 2>/dev/null || true)"
  if [ "$code" = "200" ]; then
    printf 'STADO_WEB_READY\tready\tHTTP 200 after %s attempt(s)\n' "$attempt"
    exit 0
  fi
  if [ -z "$code" ] || [ "$code" = "000" ]; then
    last='nothing answered on the port'
  else
    last="answered HTTP $code"
  fi
  if [ "$attempt" -lt "$attempts" ]; then
    /bin/sleep "$interval"
  fi
done
printf 'STADO_WEB_READY\tunready\t%s after %s attempt(s)\n' "$last" "$attempt"
"#;

/// Ask the host whether the unit answers on its own readiness path, bounded.
///
/// Returns `(verdict, detail)` rather than a bare boolean because the detail
/// is the whole value of the check: the caller turns an unready unit into a
/// refusal that names the port, the path and the last thing the host saw.
pub(in crate::cli::web::deploy) async fn wait_until_ready(
    target: &ComputeTarget,
    url: &str,
    runner: &Runner,
) -> Result<(String, String), CmdError> {
    let script = format!(
        "set -eu\nurl={}\nattempts={READY_ATTEMPTS}\ninterval={READY_INTERVAL_SECONDS}\nrequest_budget={READY_REQUEST_SECONDS}\n{WEB_READY_BODY}",
        crate::deploy::shlex_quote(url),
    );
    // The host's own worst case, plus the channel's setup: every attempt may
    // spend its full request budget and then sleep. A shorter budget here
    // would kill the probe mid-wait and report a timeout as an unready unit.
    let budget = Duration::from_secs(u64::from(
        READY_ATTEMPTS * (READY_REQUEST_SECONDS + READY_INTERVAL_SECONDS) + 30,
    ));
    let output = host_channel::run_script_with_timeout(target, &script, budget, runner)
        .await
        .map_err(click)?;
    if !output.ok() {
        return Err(CmdError::click(format!(
            "{}: the readiness probe could not be run: {}",
            target.name,
            host_channel::last_error_line(&output, "the probe reported nothing")
        )));
    }
    let fields = marker(&output.stdout, "STADO_WEB_READY").ok_or_else(|| {
        CmdError::click(format!(
            "{}: the readiness probe returned no verdict, so whether {url} answers was never \
             established",
            target.name
        ))
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
