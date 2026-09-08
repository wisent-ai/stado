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

#[cfg(test)]
mod tests {
    use super::readiness_probe_script;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::process::Command;
    use std::time::Duration;

    /// Serve one fixed JSON body on 200 to every request that arrives, on a
    /// loopback port the kernel picks, until the handle is dropped. The probe
    /// reads through `curl`, so a real socket is the only honest fixture.
    struct Health {
        port: u16,
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl Health {
        fn serving(body: &'static str) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("loopback port");
            let port = listener.local_addr().expect("bound address").port();
            listener
                .set_nonblocking(true)
                .expect("polling accept so the thread can be stopped");
            let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let flag = std::sync::Arc::clone(&stop);
            let thread = std::thread::spawn(move || {
                while !flag.load(std::sync::atomic::Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => answer(stream, body),
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(20));
                        }
                        Err(_) => return,
                    }
                }
            });
            Self {
                port,
                stop,
                thread: Some(thread),
            }
        }

        fn url(&self) -> String {
            format!("http://127.0.0.1:{}/healthz", self.port)
        }
    }

    impl Drop for Health {
        fn drop(&mut self) {
            self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    fn answer(mut stream: TcpStream, body: &str) {
        let mut request = [0u8; 1024];
        let _ = stream.read(&mut request);
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
    }

    /// A loopback port nothing is listening on: bound, its address read, then
    /// released. `curl` refuses the connection there.
    fn dead_url() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback port");
        let port = listener.local_addr().expect("bound address").port();
        drop(listener);
        format!("http://127.0.0.1:{port}/healthz")
    }

    /// Run the probe the way the host runs it and return `(ok, stderr)`.
    fn probe(url: &str, expected: Option<&str>, seconds: u64) -> (bool, String) {
        let script = readiness_probe_script(url, expected, seconds);
        let output = Command::new("/bin/bash")
            .arg("-c")
            .arg(&script)
            .output()
            .expect("bash runs the generated probe");
        (
            output.status.success(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    }

    #[test]
    fn a_url_that_never_answered_is_never_reported_as_a_missing_version_field() {
        let (ok, said) = probe(&dead_url(), Some("0.5.60"), 2);
        assert!(!ok, "a dead port cannot be ready: {said}");
        assert!(said.contains("never answered"), "{said}");
        assert!(
            !said.contains("releaseVersion"),
            "the field contract was never tested here: {said}"
        );
        assert!(
            !said.contains("build.version"),
            "the field contract was never tested here: {said}"
        );
    }

    #[test]
    fn a_service_serving_the_wrong_release_is_named_with_both_versions() {
        let health = Health::serving(r#"{"ok":true,"releaseVersion":"0.5.55"}"#);
        let (ok, said) = probe(&health.url(), Some("0.5.60"), 2);
        assert!(!ok, "0.5.55 is not the required 0.5.60: {said}");
        assert!(said.contains("reported 0.5.55"), "{said}");
        assert!(said.contains("not the required 0.5.60"), "{said}");
        assert!(!said.contains("never answered"), "it answered: {said}");
    }

    #[test]
    fn a_service_publishing_no_identity_at_all_is_named_as_that() {
        let health = Health::serving(r#"{"ok":true,"source":"weles_api"}"#);
        let (ok, said) = probe(&health.url(), Some("0.5.60"), 2);
        assert!(
            !ok,
            "an answer without an identity is not readiness: {said}"
        );
        assert!(
            said.contains("reported neither releaseVersion nor build.version"),
            "{said}"
        );
        assert!(said.contains("0.5.60 was required"), "{said}");
        assert!(!said.contains("never answered"), "it answered: {said}");
    }

    #[test]
    fn the_expected_release_passes_through_either_field() {
        let flat = Health::serving(r#"{"ok":true,"releaseVersion":"0.5.60"}"#);
        let (ok, said) = probe(&flat.url(), Some("0.5.60"), 4);
        assert!(ok, "readiness must still pass on releaseVersion: {said}");

        let nested = Health::serving(r#"{"ok":true,"build":{"version":"0.5.60"}}"#);
        let (ok, said) = probe(&nested.url(), Some("0.5.60"), 4);
        assert!(ok, "readiness must still pass on build.version: {said}");
    }

    #[test]
    fn a_probe_that_requires_no_version_is_ready_on_the_first_answer() {
        let health = Health::serving(r#"{"ok":true}"#);
        let (ok, said) = probe(&health.url(), None, 4);
        assert!(ok, "no version was required: {said}");
    }

    #[test]
    fn the_timeout_budget_is_the_one_that_was_asked_for() {
        // The sentence carries the budget the caller declared, and the probe
        // spends it: the registry's weles-worker window is 90s and a rollout
        // that reported a shorter one would be reporting someone else's gate.
        let script = readiness_probe_script("http://127.0.0.1:8788/healthz", Some("0.5.60"), 90);
        assert!(script.contains("deadline=$((SECONDS + 90))"), "{script}");
        assert_eq!(
            script.matches("readiness timed out after 90s").count(),
            3,
            "every refusal states the same budget: {script}"
        );

        let started = std::time::Instant::now();
        let (ok, said) = probe(&dead_url(), Some("0.5.60"), 2);
        assert!(!ok, "{said}");
        assert!(
            started.elapsed() >= Duration::from_secs(2),
            "the probe returned before its budget elapsed: {:?}",
            started.elapsed()
        );
    }
}
