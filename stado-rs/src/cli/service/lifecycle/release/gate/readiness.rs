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
/// answered without a version field. Three weles-worker rollouts were rolled
/// back on that sentence on 2026-09-02 while the real fault was
/// `EADDRINUSE` on the service's port, and it sent the next reader hunting a
/// field contract that was satisfiable the whole time. The three repairs are
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
