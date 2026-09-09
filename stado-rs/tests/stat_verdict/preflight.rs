//! The two refusals that happen before any verdict could exist.
//!
//! `stat` can only classify what a store said, so a store the command never
//! opened produces no receipt at all — and that is a state a caller must be
//! able to tell from `absent` just as sharply as the three unanswered
//! verdicts. Both cases here assert the same shape: a non-zero exit, no
//! receipt on stdout, and a sentence naming what has to be repaired.
//!
//! Every sentence is copied from a hand run on 2026-09-08.

use std::net::TcpListener;

use crate::fixture::{code, said, stderr, stdout, ObjectStore, OWNER_ONLY, WORLD_READABLE};
use crate::object_api::{Answer, ObjectApi};

const OBJECT: &str = "registry.json";
const RETRY_EXIT: i32 = 69;
const REFUSAL_EXIT: i32 = 1;

/// A credential a group or the world can read is refused before the request
/// is built, and the proof is that the store's socket never hears from us.
///
/// The listener is live and would answer, so nothing but the product's own
/// check can explain an empty request log.
#[test]
fn a_token_file_that_is_not_owner_only_is_refused_before_any_request_leaves() {
    let store = ObjectStore::with_token_mode(WORLD_READABLE);
    let api = ObjectApi::answering(Answer::Status(503));

    let output = store.stat(&api.url(), OBJECT);

    assert_eq!(
        store.token_mode(),
        WORLD_READABLE,
        "the case did not manage to leave a group-readable token on disk"
    );
    assert_eq!(
        code(&output),
        REFUSAL_EXIT,
        "a refused credential was not reported as one: {}",
        said(&output)
    );
    let printed = stderr(&output);
    let expected = format!(
        "Error: storage authentication failed: Stado storage token file must be owner-only \
         (chmod 600): {}",
        store.token().display()
    );
    assert_eq!(
        printed.lines().next(),
        Some(expected.as_str()),
        "the refusal did not name the file and the mode it needs: {printed}"
    );
    assert!(
        stdout(&output).trim().is_empty(),
        "a command that never opened a store printed a verdict anyway: {}",
        stdout(&output)
    );
    assert!(
        api.requests().is_empty(),
        "the credential was refused, yet these requests still left the process: {:?}",
        api.requests()
    );
}

/// A store nothing listens on is the fleet's retryable outage, and it produces
/// no verdict because no store was ever opened.
///
/// The port is the kernel's own answer: it is bound to learn a free number and
/// released again before the command runs, so the address is real and nothing
/// is behind it.
#[test]
fn a_loopback_port_nothing_listens_on_exits_with_the_retry_code_and_no_receipt() {
    let store = ObjectStore::with_token_mode(OWNER_ONLY);
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback socket to learn a port");
    let port = listener
        .local_addr()
        .expect("the kernel reports the bound address")
        .port();
    drop(listener);

    let output = store.stat(&format!("http://127.0.0.1:{port}"), OBJECT);

    assert_eq!(
        code(&output),
        RETRY_EXIT,
        "a store that cannot be reached is a retryable outage: {}",
        said(&output)
    );
    assert!(
        stdout(&output).trim().is_empty(),
        "a store that was never opened produced a verdict: {}",
        stdout(&output)
    );
    let printed = stderr(&output);
    assert!(
        printed.contains("Connection refused"),
        "the failure did not name what the kernel said: {printed}"
    );
    assert!(
        printed.contains(&format!("127.0.0.1:{port}")),
        "the failure did not name the store it could not reach: {printed}"
    );
    assert!(
        printed.contains("error_code=\"infra_down\"") && printed.contains("retryable=true"),
        "an unreachable store was not classified as our own retryable outage: {printed}"
    );
}
