//! What the product says when the body and the declaration disagree.
//!
//! Every sentence asserted here was copied from a hand run of the real binary
//! against this area's own gateway. They are the contract: an operator reads
//! them, and a reader that drops one of the two byte counts leaves them with
//! "the registry is malformed" for a transfer that never finished.

use serde_json::Value;

use crate::fixture::{stderr, stdout, Fixture};
use crate::gateway::{document, Gateway, Shape, KEY, REGISTRY_BYTES};

/// The refusal, built from the two counts that make it worth reading.
fn short_of_declared(arrived: usize, declared: usize) -> String {
    format!("Stado object API returned {arrived} of {declared} declared bytes for {KEY}")
}

/// The case the area exists for. The message ends cleanly — chunk framing
/// terminated properly — so the client's own framing check has nothing to say
/// and the read succeeds at the transport level. Only comparing the body
/// against the response's own `Content-Length` can tell that these 56 bytes
/// are not the 41,041-byte object the route said it was sending.
#[test]
fn a_body_short_of_its_declared_length_is_refused_naming_both_counts() {
    let fixture = Fixture::new();
    let body = document();
    let gateway = Gateway::start(
        Shape::Chunked {
            declared: REGISTRY_BYTES,
        },
        body.clone(),
    );

    let out = fixture.stado(&gateway, &["storage", "cat", KEY]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "a body {} bytes into a declared {REGISTRY_BYTES} is not the object",
        body.len()
    );
    assert!(
        stderr(&out).contains(&short_of_declared(body.len(), REGISTRY_BYTES)),
        "the refusal has to name both counts; got:\n{}",
        stderr(&out)
    );
    // Nothing partial is handed on. A prefix of a document on stdout is worse
    // than no document: the next program in the pipe parses it.
    assert!(
        out.stdout.is_empty(),
        "the reader emitted a partial body: {:?}",
        stdout(&out)
    );
}

/// The other direction, and it is refused by the same comparison. The body is
/// chunk-framed, so the bytes past the declaration really do arrive in this
/// process rather than being framed out of the message, and a store that sent
/// more than it declared is as untrustworthy as one that sent less.
#[test]
fn a_body_longer_than_its_declared_length_is_refused_naming_both_counts() {
    let fixture = Fixture::new();
    let body = document();
    let declared = body.len() - 1;
    let gateway = Gateway::start(Shape::Chunked { declared }, body.clone());

    let out = fixture.stado(&gateway, &["storage", "cat", KEY]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains(&short_of_declared(body.len(), declared)),
        "the refusal has to name both counts; got:\n{}",
        stderr(&out)
    );
    assert!(out.stdout.is_empty(), "got: {:?}", stdout(&out));
}

/// The shape this reader does NOT own, recorded so nobody attributes it to
/// the length comparison. A plain declared-length body whose socket closes
/// early is refused by the client's own HTTP framing, before any of this
/// area's code is reached. It is still a transport failure and still exits
/// non-zero, and if a future client change stopped enforcing framing this
/// case is where it becomes visible.
#[test]
fn an_early_close_under_a_declared_length_never_becomes_a_document() {
    let fixture = Fixture::new();
    let gateway = Gateway::start(
        Shape::ClosedEarly {
            declared: REGISTRY_BYTES,
        },
        document(),
    );

    let out = fixture.stado(&gateway, &["storage", "cat", KEY]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("end of file before message length reached"),
        "the client's own framing check owns this shape; got:\n{}",
        stderr(&out)
    );
    assert!(out.stdout.is_empty(), "got: {:?}", stdout(&out));
}

/// The operational half, and the reason the two counts matter. `storage stat`
/// answers five states and its exit code says only whether the question was
/// answered at all. A short body must land in the unanswered family: an
/// operator asking "is this object gone" while a lying gateway is in front of
/// the store has to be told the question was not answered, never that the
/// object is absent.
#[test]
fn a_short_body_leaves_the_object_unread_and_never_reported_absent() {
    let fixture = Fixture::new();
    let body = document();
    let gateway = Gateway::start(
        Shape::Chunked {
            declared: REGISTRY_BYTES,
        },
        body.clone(),
    );

    let out = fixture.stado(&gateway, &["storage", "stat", KEY, "--json"]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "an unanswered question exits non-zero: {}",
        stderr(&out)
    );
    let receipt: Value = serde_json::from_str(&stdout(&out))
        .unwrap_or_else(|error| panic!("stat prints one receipt: {error}\n{}", stdout(&out)));
    assert_eq!(
        receipt["state"], "unreachable",
        "a lying gateway is a transport failure, not an absence: {receipt:#}"
    );
    assert_eq!(
        receipt["detail"],
        Value::String(short_of_declared(body.len(), REGISTRY_BYTES))
    );
    assert_eq!(
        receipt["size"],
        Value::Null,
        "nothing is known about the object's size, so none is reported"
    );
    assert!(
        stderr(&out).contains("is UNREACHABLE, not absent"),
        "the sentence an operator reads has to say which of the five it was; got:\n{}",
        stderr(&out)
    );

    // Both reads the probe makes hit this socket: the versioned read, and the
    // binary re-probe that exists so a non-UTF-8 body is not mistaken for an
    // unreachable store. Both have to refuse, or the second would quietly
    // rescue a body the first correctly rejected.
    let reads = gateway.object_reads();
    assert_eq!(
        reads.len(),
        2,
        "got {:?}",
        reads.iter().map(|ask| &ask.target).collect::<Vec<_>>()
    );
    assert!(
        reads[0].target.contains("versioned=true"),
        "the first probe is the versioned read: {:?}",
        reads[0].target
    );
    assert!(
        !reads[1].target.contains("versioned=true"),
        "the second probe is the plain binary read: {:?}",
        reads[1].target
    );
}
