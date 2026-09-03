//! `stado storage stat` tells apart the ways a store can fail to answer.
//!
//! Three of the five verdicts used to be one word. A `401` refusal, a `503`
//! boundary that is down and the resolver's own `502 upstream unavailable` all
//! arrived as `unreachable`, separable only by reading a prose detail line —
//! so a caller asking "is this coordinate spent" could not tell a credential
//! it must repair from an outage it should wait out from a transport it should
//! chase. Two releases turned on that question on 2026-09-03 and got the same
//! word for all three causes.
//!
//! Every test here drives the built `stado` binary against a fake release
//! channel that answers one fixed status, so the verdict under test is the one
//! the product computes from a real HTTP answer. `STADO_API_URL` is the only
//! configuration needed: `RemoteObjectApi::endpoint_from_env_or_config` reads
//! it before it consults any backend, and the `stado://releases/...` route
//! never touches the job store.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, Output};

/// A published release coordinate, spelled with its scheme so the namespace is
/// explicit and the release channel is the store that gets asked.
const RELEASE_URI: &str = "stado://releases/stado/0.13.20/darwin-arm64/SHA256SUMS";

/// A loopback server that answers every request with one status line and
/// closes, for as long as the returned handle lives.
///
/// It is deliberately not an HTTP library: the product only reads the status
/// and, when the status is a success, `Content-Length`, and hand-writing the
/// answer is what lets a test name a status no library would let it send.
struct Channel {
    port: u16,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Channel {
    fn answering(status_line: &'static str, extra_headers: &'static str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback bind");
        let port = listener.local_addr().expect("bound address").port();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = stop.clone();
        std::thread::spawn(move || {
            for accepted in listener.incoming() {
                if flag.load(std::sync::atomic::Ordering::Relaxed) {
                    return;
                }
                let Ok(mut stream) = accepted else { return };
                // Read just enough to let the client finish writing its
                // request; a server that answers without draining can make the
                // client see a broken pipe instead of the status under test.
                let mut head = [0_u8; 2048];
                let _ = stream.read(&mut head);
                let _ = stream.write_all(
                    format!("HTTP/1.1 {status_line}\r\n{extra_headers}Connection: close\r\n\r\n")
                        .as_bytes(),
                );
                let _ = stream.flush();
            }
        });
        Self { port, stop }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}/", self.port)
    }
}

impl Drop for Channel {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        // Unblock the accept loop so the thread observes the flag.
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

/// `stado storage stat --json` against one fake channel.
///
/// `HOME` is a temp dir and `STADO_CONFIG` is set-but-missing, so the
/// developer's real configuration cannot decide what this test measures.
fn stat(home: &Path, url: &str, uri: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(["storage", "stat", uri, "--json"])
        .env("HOME", home)
        .env("STADO_CONFIG", home.join("no-such-config.json"))
        .env("STADO_API_URL", url)
        .env_remove("STADO_API_TOKEN")
        .env_remove("COMPUTE_API_KEY")
        .env_remove("COMPUTE_API_URL")
        .output()
        .expect("stado binary runs")
}

/// The receipt's `state`, which is the field a script branches on.
fn state(out: &Output) -> String {
    let stdout = String::from_utf8_lossy(&out.stdout);
    let receipt: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|error| {
        panic!(
            "stat did not print a receipt ({error}): {stdout}{}",
            String::from_utf8_lossy(&out.stderr)
        )
    });
    receipt["state"]
        .as_str()
        .unwrap_or_else(|| panic!("receipt carries no state: {receipt}"))
        .to_string()
}

fn said(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// A store that answers "you may not ask" is `refused`, not `unreachable`.
///
/// The store DID answer, and it answered about this reader's standing rather
/// than about the object. Retrying it unchanged cannot learn anything, which
/// is the opposite of what `unreachable` invites a caller to do.
#[test]
fn a_refused_question_is_not_an_unreachable_store() {
    let home = tempfile::tempdir().unwrap();
    let channel = Channel::answering("401 Unauthorized", "");
    let out = stat(home.path(), &channel.url(), RELEASE_URI);

    assert_eq!(
        state(&out),
        "refused",
        "a 401 was not reported as a refusal: {}",
        said(&out)
    );
    assert!(
        !out.status.success(),
        "a refused question must not exit zero: it was not answered. {}",
        said(&out)
    );
}

/// A store that answers "not right now" is `unavailable`, not `unreachable`.
///
/// This is the object plane's own `503 object authorization unavailable` when
/// its Skarbiec boundary is down: a wait, not a dead store, and the caller
/// that can distinguish them is the one that retries instead of failing a
/// release.
#[test]
fn a_temporary_refusal_is_not_an_unreachable_store() {
    let home = tempfile::tempdir().unwrap();
    let channel = Channel::answering("503 Service Unavailable", "");
    let out = stat(home.path(), &channel.url(), RELEASE_URI);

    assert_eq!(
        state(&out),
        "unavailable",
        "a 503 was not reported as temporarily unavailable: {}",
        said(&out)
    );
    assert!(
        !out.status.success(),
        "an unavailable store must not exit zero: it was not answered. {}",
        said(&out)
    );
}

/// A proxy's `502` stays `unreachable`, because nothing answered.
///
/// Stado's own service resolver writes exactly `502 upstream unavailable` when
/// its SSH forward cannot carry a connection. The store never saw the
/// question, so this is the one status of the three that genuinely means "I
/// could not look", and splitting the verdicts must not steal it.
#[test]
fn a_gateway_failure_is_still_unreachable() {
    let home = tempfile::tempdir().unwrap();
    let channel = Channel::answering("502 Bad Gateway", "");
    let out = stat(home.path(), &channel.url(), RELEASE_URI);

    assert_eq!(
        state(&out),
        "unreachable",
        "a 502 lost its unreachable verdict: {}",
        said(&out)
    );
    assert!(
        !out.status.success(),
        "an unreachable store must not exit zero. {}",
        said(&out)
    );
}

/// The contract the split must not break: `absent` stays an ANSWER.
///
/// A caller asking whether a coordinate is spent reads the exit status, and
/// zero has to keep meaning "the store answered". If any unanswered state ever
/// exits zero, a dead store reads as a drained one.
#[test]
fn an_explicit_absence_is_answered_and_exits_zero() {
    let home = tempfile::tempdir().unwrap();
    let channel = Channel::answering("404 Not Found", "");
    let out = stat(home.path(), &channel.url(), RELEASE_URI);

    assert_eq!(
        state(&out),
        "absent",
        "a 404 was not reported as absence: {}",
        said(&out)
    );
    assert!(
        out.status.success(),
        "an answered question must exit zero, or absence is indistinguishable \
         from silence. {}",
        said(&out)
    );
}

/// And a served object is `present` and exits zero, so the four verdicts above
/// are measured against a channel that can also say yes.
#[test]
fn a_served_object_is_present_and_exits_zero() {
    let home = tempfile::tempdir().unwrap();
    let channel = Channel::answering("200 OK", "Content-Length: 0\r\n");
    let out = stat(home.path(), &channel.url(), RELEASE_URI);

    assert_eq!(
        state(&out),
        "present",
        "a 200 was not reported as present: {}",
        said(&out)
    );
    assert!(
        out.status.success(),
        "a present object failed: {}",
        said(&out)
    );
}

/// Each unanswered verdict names its own remedy, so an operator reading the
/// refusal is told what to do rather than being told only that something went
/// wrong.
///
/// This is the half of the defect a `state` field alone does not fix: the
/// three states used to share one sentence, and that sentence said "the store
/// did not answer" even when the store had answered.
#[test]
fn each_unanswered_verdict_names_its_own_remedy() {
    let home = tempfile::tempdir().unwrap();

    let refused = Channel::answering("403 Forbidden", "");
    let out = stat(home.path(), &refused.url(), RELEASE_URI);
    let sentence = said(&out);
    assert!(
        sentence.contains("REFUSED") && sentence.contains("credential"),
        "a refusal did not name the credential as its remedy: {sentence}"
    );
    drop(refused);

    let unavailable = Channel::answering("429 Too Many Requests", "");
    let out = stat(home.path(), &unavailable.url(), RELEASE_URI);
    let sentence = said(&out);
    assert!(
        sentence.contains("UNAVAILABLE") && sentence.contains("retry"),
        "a temporary failure did not name retrying as its remedy: {sentence}"
    );
    drop(unavailable);

    let unreachable = Channel::answering("504 Gateway Timeout", "");
    let out = stat(home.path(), &unreachable.url(), RELEASE_URI);
    let sentence = said(&out);
    assert!(
        sentence.contains("UNREACHABLE") && sentence.contains("transport"),
        "an unreachable store did not name the transport as its remedy: {sentence}"
    );
}

/// The split has to agree with the retryability contract, or a caller that
/// believes the verdict and a caller that believes the failure envelope
/// disagree about the same failure.
///
/// A refused question is `auth`, `retryable=false`: asking again cannot help.
/// A temporarily unavailable store is retryable. Those codes come from
/// [`crate::failure`] reading the HTTP status out of the message, which is why
/// each verdict's detail keeps the status it was computed from rather than
/// replacing it with the verdict's own word.
///
/// This is the half of the defect that cost a peer real time tonight: reading
/// `unreachable` for a `401`, they spent retries on a permission error in the
/// belief that it was a transport race.
#[test]
fn a_refused_verdict_is_not_retryable_and_an_unavailable_one_is() {
    let home = tempfile::tempdir().unwrap();

    // Both halves are asserted together on purpose. The envelope reads the
    // status and the verdict reads the state, so checking only the envelope
    // would pass while the two disagreed -- which is exactly the state this
    // fixes: the classifier already said `auth, retryable=false` for a 401
    // while `stat` printed `unreachable` over the top of it.
    let refused = Channel::answering("401 Unauthorized", "");
    let out = stat(home.path(), &refused.url(), RELEASE_URI);
    let verdict = state(&out);
    let sentence = said(&out);
    assert_eq!(
        verdict, "refused",
        "the verdict disagrees with the envelope, which classified this: {sentence}"
    );
    assert!(
        sentence.contains("error_code=\"auth\"") && sentence.contains("retryable=false"),
        "a refused question was not classified as a non-retryable auth failure: {sentence}"
    );
    drop(refused);

    let unavailable = Channel::answering("503 Service Unavailable", "");
    let out = stat(home.path(), &unavailable.url(), RELEASE_URI);
    let verdict = state(&out);
    let sentence = said(&out);
    assert_eq!(
        verdict, "unavailable",
        "the verdict disagrees with the envelope, which classified this: {sentence}"
    );
    assert!(
        sentence.contains("retryable=true"),
        "a temporarily unavailable store was not classified as retryable: {sentence}"
    );
}
