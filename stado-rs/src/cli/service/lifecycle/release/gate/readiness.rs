//! The readiness gate: the host-side probe, and what each way of failing it
//! says.

use super::*;

fn validate_readiness_url(url: &str) -> Result<(), CmdError> {
    if ["http://127.0.0.1:", "http://localhost:", "http://[::1]:"]
        .iter()
        .any(|prefix| url.starts_with(prefix))
        && !url.chars().any(char::is_whitespace)
    {
        return Ok(());
    }
    Err(CmdError::usage(
        "--readiness-url must be a whitespace-free loopback HTTP URL",
    ))
}

/// The host-side readiness probe, as a script, separated from running it so
/// its refusals can be exercised directly.
///
/// Three distinct failures used to share one sentence. A timeout appended
/// `did not report releaseVersion or build.version <expected>` whenever a
/// version was required, including when `curl` never once succeeded — so a
/// candidate whose HTTP server never bound its port was reported as one that
/// answered without a version field. Rollouts get rolled back on that
/// sentence while the real fault is `EADDRINUSE` on the service's port, and
/// it sends the next reader hunting a field contract that was satisfiable
/// the whole time. The three repairs are
/// different — start the service, publish the release the gate asked for, or
/// teach the service to report its identity — so the sentence names which
/// one is owed:
///
/// - the URL never answered,
/// - it answered and reported a value that is not the expected one,
/// - it answered with neither field present.
///
/// `expected` empty means no version is required, and then the first answer
/// is readiness, so a timeout in that mode can only be the first case.
fn readiness_probe_script(
    url: &str,
    expected_release_version: Option<&str>,
    timeout_seconds: u64,
) -> String {
    format!(
        "set -euo pipefail\nurl={url}\nexpected={expected}\n\
         deadline=$((SECONDS + {timeout}))\n\
         answered=no\n\
         reported=\n\
         while [ \"$SECONDS\" -lt \"$deadline\" ]; do\n\
           if body=$(/usr/bin/curl -fsS --max-time 2 \"$url\"); then\n\
             answered=yes\n\
             reported=\n\
             if [ -n \"$expected\" ]; then\n\
               reported=\"$(printf '%s' \"$body\" | /usr/bin/plutil -extract releaseVersion raw -o - - 2>/dev/null)\" || reported=\n\
               if [ -z \"$reported\" ]; then\n\
                 reported=\"$(printf '%s' \"$body\" | /usr/bin/plutil -extract build.version raw -o - - 2>/dev/null)\" || reported=\n\
               fi\n\
             fi\n\
             if [ -z \"$expected\" ] || [ \"$reported\" = \"$expected\" ]; then\n\
               printf '%s\\n' ready\n\
               exit 0\n\
             fi\n\
           fi\n\
           /bin/sleep 1\n\
         done\n\
         if [ \"$answered\" = no ]; then\n\
           detail=\"readiness timed out after {timeout}s: $url never answered\"\n\
         elif [ -z \"$reported\" ]; then\n\
           detail=\"readiness timed out after {timeout}s: $url answered, and reported neither releaseVersion nor build.version, where $expected was required\"\n\
         else\n\
           detail=\"readiness timed out after {timeout}s: $url answered, and reported $reported, not the required $expected\"\n\
         fi\n\
         printf '%s\\n' \"$detail\" >&2\n\
         exit 1",
        url = crate::deploy::shlex_quote(url),
        expected = crate::deploy::shlex_quote(expected_release_version.unwrap_or_default()),
        timeout = timeout_seconds,
    )
}

pub(super) async fn wait_for_service_readiness(
    target: &targets::ComputeTarget,
    url: &str,
    expected_release_version: Option<&str>,
    timeout_seconds: u64,
    runner: &crate::deploy::Runner,
) -> Result<(), CmdError> {
    validate_readiness_url(url)?;
    if timeout_seconds == 0 || timeout_seconds > 600 {
        return Err(CmdError::usage(
            "--readiness-timeout-seconds must be between 1 and 600",
        ));
    }
    let script = readiness_probe_script(url, expected_release_version, timeout_seconds);
    let output = host_channel::run_script(target, &script, runner)
        .await
        .map_err(click)?;
    if output.ok() {
        Ok(())
    } else {
        Err(CmdError::click(host_channel::last_error_line(
            &output,
            "readiness failed",
        )))
    }
}

/// The work the running service says a restart would end: its readiness
/// answer's `in_flight` list, read once, on the host.
///
/// A replace restarts the unit, and a restart ends whatever the process was
/// doing. Weles landed releases while a sign-in waited on the operator's
/// phone, and each one killed the run mid-wait. A service that names its
/// in-flight work under `in_flight` in its readiness answer is restarted only
/// once that list is empty; one that names none, does not answer, or answers
/// something that is not JSON has nothing the delivery can see to lose, and is
/// restarted as before. The read waits at most the policy's own readiness
/// window, the bound this gate already applies to the same URL.
pub(super) async fn in_flight_work(
    target: &targets::ComputeTarget,
    url: &str,
    timeout_seconds: u64,
    runner: &crate::deploy::Runner,
) -> Result<Vec<String>, CmdError> {
    validate_readiness_url(url)?;
    let script = format!(
        "/usr/bin/curl -fsS --max-time {timeout} {url} || true",
        timeout = timeout_seconds,
        url = crate::deploy::shlex_quote(url),
    );
    let output = host_channel::run_script(target, &script, runner)
        .await
        .map_err(click)?;
    Ok(in_flight_from_answer(&output.stdout))
}

/// Each in-flight entry of a readiness answer, as one line naming it.
fn in_flight_from_answer(body: &str) -> Vec<String> {
    let Ok(answer) = serde_json::from_str::<serde_json::Value>(body.trim()) else {
        return Vec::new();
    };
    answer
        .get("in_flight")
        .and_then(serde_json::Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|entry| match entry {
                    serde_json::Value::Object(fields) => fields
                        .iter()
                        .map(|(key, value)| match value {
                            serde_json::Value::String(text) => format!("{key}={text}"),
                            other => format!("{key}={other}"),
                        })
                        .collect::<Vec<_>>()
                        .join(" "),
                    serde_json::Value::String(text) => text.clone(),
                    other => other.to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}
