//! The sibling route, and the reader that was not comparing anything.
//!
//! `/api/object` is not the only route this backend reads bodies off.
//! `BlobBackend::download_release` reads `/api/release/object`, which is how
//! a published software archive reaches a host, and it returned
//! `response.bytes()` with nothing compared against the length the route had
//! just declared. That reader is not reachable from a command — its callers
//! are the job-input materializer and the dashboard's own object plane — so
//! it is driven here through the crate's public backend against the same real
//! socket every other case in this area uses.
//!
//! A release archive is not a document that fails to parse when it is short.
//! It is an archive that unpacks into a truncated tree, or a binary that
//! runs, and the length the route declared was the only thing on the wire
//! that could have said so.

use stado::queue::{BlobBackend, StadoObjectBackend, StorageError};

use crate::fixture::{Fixture, NAMESPACE};
use crate::gateway::{document, Gateway, Shape, REGISTRY_BYTES};

/// A published archive's coordinate, in the shape the release channel serves.
const RELEASE_URI: &str = "stado://releases/stado/0.16.43/stado-0.16.43-darwin-arm64.tar.gz";

fn backend(fixture: &Fixture, gateway: &Gateway) -> StadoObjectBackend {
    StadoObjectBackend::new(
        gateway.origin(),
        NAMESPACE,
        fixture
            .token_file()
            .to_str()
            .expect("the temporary path is UTF-8"),
        "",
    )
    .expect("a loopback origin with an owner-only bearer is a valid store")
}

#[tokio::test]
async fn a_short_release_body_is_refused_naming_both_counts() {
    let fixture = Fixture::new();
    let body = document();
    let gateway = Gateway::start(
        Shape::Chunked {
            declared: REGISTRY_BYTES,
        },
        body.clone(),
    );

    let error = backend(&fixture, &gateway)
        .download_release(RELEASE_URI)
        .await
        .expect_err("a body short of the length the release route declared is not the archive");
    assert!(
        matches!(error, StorageError::Other(_)),
        "a short transfer belongs to the transport family, not to a parse class; got {error:?}"
    );
    let message = error.to_string();
    assert!(
        message.contains(&body.len().to_string()) && message.contains(&REGISTRY_BYTES.to_string()),
        "the refusal has to say how short it was; got {message:?}"
    );
    assert!(
        message.contains(RELEASE_URI),
        "the refusal has to name the object; got {message:?}"
    );

    // The read really went to the release route on this socket, which is the
    // whole reason this case is separate from the object-route ones.
    let reads = gateway.object_reads();
    assert_eq!(reads.len(), 1, "got {reads:?}");
    assert!(
        reads[0].target.starts_with("/api/release/object?uri="),
        "got {:?}",
        reads[0].target
    );
}

/// The same route, answering honestly. A whole archive still arrives whole,
/// so the comparison above cannot be paid for by breaking delivery.
#[tokio::test]
async fn a_whole_release_body_is_delivered_unchanged() {
    let fixture = Fixture::new();
    let body = document();
    let gateway = Gateway::start(Shape::Whole, body.clone());

    let delivered = backend(&fixture, &gateway)
        .download_release(RELEASE_URI)
        .await
        .expect("a whole body is accepted")
        .expect("the gateway answered 200, so the object exists");
    assert_eq!(delivered, body);
}
