//! A registry read whose authority accepts the SSH connection and answers
//! nothing ends on its own, after the refresh interval this host's resolver
//! declares, refused with the interval and where to ask, and the reader
//! goes on to its last-known-good copy instead of waiting without end.
//!
//! The real `stado registry self` runs with an isolated `HOME` under the
//! test's output directory and a last-known-good registry copy that names
//! this machine as the reader and, as the registry authority, a real OpenSSH
//! server on a loopback port that runs the real stado (`hung_ssh.rs`). The
//! authority answers one read with its own refusal and is then stopped with
//! SIGSTOP: its connections still complete and no SSH banner ever comes, so
//! the authority's `stado resolver snapshot` can never be asked and nothing
//! of this host's resolver records the wait. The test holds no clock: the
//! command must end by itself.
#[path = "../hung_ssh.rs"]
mod hung_ssh;
mod run;

use serde_json::{json, Value};

use run::{connecting, hostname, Run, ABANDONED, AUTHORITY};

#[test]
fn a_silent_authority_is_abandoned_after_the_declared_refresh_interval() {
    let mut run = Run::new("registry-self");
    let (authority_ssh, refresh_seconds) = run.hung_authority();
    let output = run.registry_self("registry-self");
    let said = String::from_utf8_lossy(&output.stderr).into_owned();

    assert!(
        said.contains(ABANDONED)
            && said.contains(AUTHORITY)
            && said.contains(&format!(
                "it stood longer than the {refresh_seconds}s refresh interval this host's resolver declares"
            ))
            && said.contains("`stado resolver snapshot` is what answered nothing")
            && said.contains(&format!("stado service logs com.wisent.stado --host {AUTHORITY}")),
        "the read is abandoned naming the authority, the interval it outlasted and where to ask: {said}"
    );
    assert!(
        said.contains("reading the last-known-good registry copy"),
        "the reader goes on to its last-known-good copy: {said}"
    );
    assert!(
        said.contains(&connecting(&authority_ssh)),
        "the read was abandoned on a connection it opened to the stopped authority: {said}"
    );
    std::fs::write(
        run.root.join("report.json"),
        serde_json::to_vec_pretty(&json!({
            "authority_ssh": authority_ssh,
            "authority_log": run.authority_log(),
            "refresh_seconds": refresh_seconds,
            "exit_status": output.status.to_string(),
            "stderr": said,
            "stdout": String::from_utf8_lossy(&output.stdout),
        }))
        .unwrap(),
    )
    .unwrap();
}

/// A resolver state left on disk that names no refresh interval — the
/// state a resolver that stopped, or one built before it published the
/// interval, leaves behind — is no reason to wait without end: the read is
/// measured by the interval this machine's registry copy declares, as is
/// every read an install's converge makes this way.
#[test]
fn a_resolver_state_naming_no_interval_still_bounds_the_read() {
    let mut run = Run::new("stale-state");
    let (_, refresh_seconds) = run.hung_authority();
    std::fs::write(
        run.root.join(".stado/resolver-state.json"),
        serde_json::to_vec(&json!({
            "updated_at": "1970-01-01T00:00:00Z",
            "target": hostname(),
            "state": "serving",
            "store_version": "last-known-good",
            "listening": true,
        }))
        .unwrap(),
    )
    .unwrap();
    let output = run.registry_self("stale-state");
    let said = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        said.contains(ABANDONED)
            && said.contains(&format!("the {refresh_seconds}s refresh interval")),
        "the read is abandoned at the interval the registry copy declares: {said}"
    );
    assert!(
        said.contains("reading the last-known-good registry copy"),
        "the reader goes on to its last-known-good copy: {said}"
    );
}

/// The next reader of an authority a read already gave up on is not made to
/// wait that interval again: it is told at once, with the earlier read's
/// sentence, and goes on to its copy, as `stado host ping` and every agent's
/// call are.
#[test]
fn a_second_reader_is_told_at_once_that_the_authority_went_unanswered() {
    let mut run = Run::new("second-reader");
    let (authority_ssh, _) = run.hung_authority();
    let first = run.registry_self("first-reader");
    let first_said = String::from_utf8_lossy(&first.stderr).into_owned();
    assert!(
        first_said.contains(ABANDONED),
        "the first read waits and is abandoned: {first_said}"
    );
    let second = run.registry_self("second-reader");
    let said = String::from_utf8_lossy(&second.stderr).into_owned();
    assert!(
        said.contains(&format!(
            "the registry authority {AUTHORITY} is not asked again before"
        )) && said.contains(ABANDONED),
        "the second read is refused at once with the first read's sentence: {said}"
    );
    assert!(
        said.contains("reading the last-known-good registry copy"),
        "the second reader goes on to its copy: {said}"
    );
    assert!(
        !said.contains(&connecting(&authority_ssh)),
        "the second read opened no connection to the authority: {said}"
    );
    assert!(run.root.join(".stado/authority-unanswered.json").is_file());
}

/// `stado resolver status` says how long the authority's snapshot fetch
/// took, so a fetch the authority never answered is not read as a
/// reachable authority: against the silent authority the fetch is abandoned
/// at the declared refresh interval, and the answer carries that wait in
/// seconds, in JSON and on the text authority line.
#[test]
fn resolver_status_says_how_long_the_authority_took_to_answer() {
    let mut run = Run::new("resolver-status");
    let (authority_ssh, refresh_seconds) = run.hung_authority();
    let json_answer = run
        .stado()
        .args(["resolver", "status", "--json"])
        .output()
        .expect("run the real stado");
    let text_answer = run
        .stado()
        .args(["resolver", "status"])
        .output()
        .expect("run the real stado");
    std::fs::write(
        run.root.join("resolver-status-report.json"),
        serde_json::to_vec_pretty(&json!({
            "authority_ssh": authority_ssh,
            "refresh_seconds": refresh_seconds,
            "json": {
                "exit_status": json_answer.status.to_string(),
                "stdout": String::from_utf8_lossy(&json_answer.stdout),
                "stderr": String::from_utf8_lossy(&json_answer.stderr),
            },
            "text": {
                "exit_status": text_answer.status.to_string(),
                "stdout": String::from_utf8_lossy(&text_answer.stdout),
                "stderr": String::from_utf8_lossy(&text_answer.stderr),
            },
        }))
        .unwrap(),
    )
    .unwrap();
    let status: Value = match serde_json::from_slice(&json_answer.stdout) {
        Ok(status) => status,
        Err(error) => panic!(
            "stado resolver status --json answered no JSON ({error}): {}",
            String::from_utf8_lossy(&json_answer.stderr)
        ),
    };
    let authority = &status["authority"];
    assert_eq!(authority["target"], AUTHORITY, "{status}");
    assert_eq!(authority["source"], "ssh", "{status}");
    assert_eq!(
        authority["reachable"], false,
        "a silent authority read as reachable: {status}"
    );
    let Some(waited) = authority["answered_after_seconds"].as_i64() else {
        panic!("the answer does not say how long the authority's fetch took: {status}");
    };
    assert!(
        waited >= refresh_seconds,
        "the fetch was abandoned at the {refresh_seconds}s refresh interval, yet the answer says {waited}s: {status}"
    );
    let text = String::from_utf8_lossy(&text_answer.stdout);
    let Some(line) = text
        .lines()
        .find(|line| line.starts_with(&format!("authority {AUTHORITY} source=ssh")))
    else {
        panic!("the text answer has no authority line: {text}");
    };
    assert!(
        line.contains(" answered_after=") && line.ends_with('s'),
        "the authority line does not say how long the fetch took: {line}"
    );
}
