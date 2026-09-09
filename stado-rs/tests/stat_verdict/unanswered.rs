//! The three verdicts that are NOT answers, through the loopback store.
//!
//! Each one is something the caller can act on, and they used to be one word:
//! a `401` refusal, a `503` boundary that is down, and a transport that died
//! all arrived as `unreachable`, separable only by reading prose. A caller
//! asking "is this coordinate spent?" cannot branch on prose, so each case
//! here pins the verdict, the exit code, the detail the verdict was computed
//! from, and the classification the failure envelope gave the same failure.
//!
//! Every sentence is copied from a hand run on 2026-09-08.

use crate::fixture::{code, receipt, said, state, stderr, stdout, ObjectStore, OWNER_ONLY};
use crate::object_api::{Answer, ObjectApi};

/// The object the cases ask about. The store answers about it or fails to; it
/// is never served.
const OBJECT: &str = "registry.json";

/// `EX_UNAVAILABLE`, the fleet-wide "worth retrying later" exit code, and the
/// generic failure code. Both were read off live runs of this command.
const RETRY_EXIT: i32 = 69;
const REFUSAL_EXIT: i32 = 1;

/// How many times the probe reads the object: the versioned read, and the
/// byte re-probe behind it that tells a non-UTF-8 body from a store that
/// cannot answer.
const OBJECT_READS: usize = 2;

/// A store that answers "not right now" is `unavailable`, and above all it is
/// not `absent`.
///
/// This is the distinction the whole area exists for: the object plane answers
/// `503` while its authorization boundary revalidates, and a caller that reads
/// that as absence deletes or republishes an object that is still there.
#[test]
fn a_store_answering_503_is_unavailable_and_never_absent() {
    let store = ObjectStore::with_token_mode(OWNER_ONLY);
    let api = ObjectApi::answering(Answer::Status(503));

    let output = store.stat(&api.url(), OBJECT);
    let receipt = receipt(&output);

    assert_eq!(
        receipt["state"],
        "unavailable",
        "a store that answered 503 was not reported unavailable: {}",
        said(&output)
    );
    assert_ne!(
        receipt["state"], "absent",
        "a store that could not answer was reported as an answer: {receipt}"
    );
    assert_eq!(
        code(&output),
        RETRY_EXIT,
        "an unanswered question must not exit zero, and a retryable one carries \
         the retry code: {}",
        said(&output)
    );
    assert_eq!(
        receipt["detail"].as_str(),
        Some(
            "Stado object API error HTTP 503: {\"error\":\"this store is not serving reads right \
             now\"}"
        ),
        "the verdict dropped the answer it was computed from: {receipt}"
    );

    let printed = stderr(&output);
    assert!(
        printed.contains(
            "\"registry.json\" is UNAVAILABLE, not absent — the store answered that it cannot \
             answer right now: this same question may be answered later, so retry it"
        ),
        "the refusal did not name retrying as its remedy: {printed}"
    );
    assert!(
        printed.contains("error_code=\"infra_down\"") && printed.contains("retryable=true"),
        "the envelope and the verdict disagree about the same failure: {printed}"
    );

    // The store really was asked: the object read left the process and reached
    // this socket, twice — the versioned read and the byte re-probe behind it.
    assert_eq!(
        api.object_requests().len(),
        OBJECT_READS,
        "the object was not read the way the verdict claims: {:?}",
        api.requests()
    );
}

/// A store that answers "you may not ask" is `refused`, and the failure
/// envelope has to agree that asking again cannot help.
#[test]
fn a_store_answering_401_is_refused_and_not_retryable() {
    let store = ObjectStore::with_token_mode(OWNER_ONLY);
    let api = ObjectApi::answering(Answer::Status(401));

    let output = store.stat(&api.url(), OBJECT);

    assert_eq!(
        state(&output),
        "refused",
        "a store that answered 401 was not reported refused: {}",
        said(&output)
    );
    assert_eq!(
        code(&output),
        REFUSAL_EXIT,
        "a refused question was not reported as a failure of this reader's \
         standing: {}",
        said(&output)
    );

    let printed = stderr(&output);
    assert!(
        printed.contains(
            "\"registry.json\" is REFUSED, not absent — the store answered that this reader may \
             not ask: repair the credential or the grant, because the same question asked again \
             cannot learn anything"
        ),
        "the refusal did not name the credential as its remedy: {printed}"
    );
    assert!(
        printed.contains("error_code=\"auth\"") && printed.contains("retryable=false"),
        "a refusal was classified as something a retry could fix: {printed}"
    );
    assert_eq!(
        receipt(&output)["detail"].as_str(),
        Some(
            "Stado object API error HTTP 401: {\"error\":\"this store is not serving reads right \
             now\"}"
        ),
        "the verdict dropped the answer it was computed from: {}",
        said(&output)
    );
}

/// A transport that dies mid-read stays `unreachable`, because nothing
/// answered.
///
/// The socket takes the whole request and then closes without a reply, which
/// is what a forward whose channel has gone cold does. Splitting the other two
/// verdicts out must not steal this one: it is the only state that means "I
/// could not look".
#[test]
fn a_socket_that_hangs_up_without_answering_is_unreachable() {
    let store = ObjectStore::with_token_mode(OWNER_ONLY);
    let api = ObjectApi::answering(Answer::HangUp);

    let output = store.stat(&api.url(), OBJECT);
    let receipt = receipt(&output);

    assert_eq!(
        receipt["state"],
        "unreachable",
        "a store that never answered was reported as something it did not say: {}",
        said(&output)
    );
    assert_eq!(
        code(&output),
        RETRY_EXIT,
        "a transport failure is worth retrying and must carry the retry code: {}",
        said(&output)
    );
    assert!(
        receipt["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("connection closed before message completed")),
        "the verdict did not carry what the transport did: {receipt}"
    );
    assert!(
        stderr(&output).contains(
            "is UNREACHABLE, not absent — nothing answered at all: chase the transport in front \
             of the store"
        ),
        "the refusal did not name the transport as its remedy: {}",
        stderr(&output)
    );
    assert!(
        !stdout(&output).is_empty(),
        "an unanswered question still owes the caller a receipt to branch on"
    );
}
