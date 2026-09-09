//! A short object body is a transport failure, not a document.
//!
//! The Stado object route answers every read with a `Content-Length`, and the
//! body goes straight on to a parser: the canonical registry is read through
//! `RegistryStore` and handed to `serde_json::from_str`. A reader that
//! returns `response.bytes()` without comparing it against the declaration
//! therefore turns a transfer that stopped short into a document that does
//! not parse — a finding about the registry's CONTENT, journalled where a
//! transport failure belonged. This area's job is to make the two
//! distinguishable, in the product's own words and exit codes.
//!
//! Commit `021dfb38` deleted this area whole, on the grounds that a body
//! shorter than its `Content-Length` is a state only a lying server produces.
//! That is true and it is the point: the store IS the suspect here, and
//! nothing rebuilt the coverage. What stands here now is a real one. The
//! gateway is a `TcpListener` this process binds on `127.0.0.1:0` and answers
//! HTTP/1.1 on by hand — a conforming server cannot be asked to contradict
//! its own framing, so the framing is written out here rather than delegated.
//! It is declared as the store through `WC_STADO_STORAGE_URL`, an owner-only
//! `WC_STADO_STORAGE_TOKEN_FILE` and `WC_STADO_STORAGE_NAMESPACE`, and the
//! real `stado` binary connects to it over TCP.
//!
//! Three shapes, and what the product does with each, every sentence below
//! copied from a hand run against this same socket:
//!
//! * a whole body is delivered byte for byte and exits `0`;
//! * a body short of the length it declares is refused, naming both counts,
//!   and `storage stat` calls it `unreachable` rather than `absent`;
//! * a body longer than the length it declares is refused the same way.
//!
//! One framing is deliberately not a case. A plain `Content-Length` body with
//! extra bytes after it is not an over-long message: the declared count IS
//! the message, and the excess is never delivered to any conforming client,
//! so there is nothing for the product to notice and nothing to assert. The
//! over-long case below is therefore chunk-framed, which is the framing in
//! which those bytes really do arrive.
//!
//! Isolation: one `TempDir` per case holds the bearer and HOME, the child's
//! environment is cleared and rebuilt, and `STADO_CONFIG` names a path that
//! does not exist. Nothing here reaches the operator's real gateway, store or
//! registry.

mod fixture;
mod gateway;
mod refusals;
mod release;

use fixture::{stderr, stdout, Fixture};
use gateway::{document, Gateway, Shape, KEY};

/// The whole path, on a store that behaves: the product asks this socket for
/// the object, with the bearer this test wrote, and hands back exactly the
/// bytes the socket sent.
#[test]
fn a_whole_body_reaches_the_reader_and_this_socket_is_where_it_came_from() {
    let fixture = Fixture::new();
    let body = document();
    let gateway = Gateway::start(Shape::Whole, body.clone());

    let out = fixture.stado(&gateway, &["storage", "cat", KEY]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "a body that matches its declared length is the object: {}",
        stderr(&out)
    );
    assert_eq!(
        out.stdout, body,
        "the reader delivered something other than the bytes this socket sent"
    );

    // The state the read left on the far side. Stdout could in principle come
    // from anywhere; this is the gateway's own record that the product opened
    // a connection to the declared store, presented the bearer from the
    // owner-only file, and asked for this object by its namespaced URI.
    let reads = gateway.object_reads();
    assert_eq!(
        reads.len(),
        1,
        "one read of the object, got {:?}",
        reads.iter().map(|ask| &ask.target).collect::<Vec<_>>()
    );
    assert!(
        reads[0].target.starts_with("/api/object?uri="),
        "the object route is where a queue object is read: {:?}",
        reads[0].target
    );
    assert!(
        reads[0]
            .target
            .contains(&format!("{}%2F{KEY}", fixture::NAMESPACE)),
        "the read named the wrong object: {:?}",
        reads[0].target
    );
    assert_eq!(
        reads[0].authorization.as_deref(),
        Some(format!("Bearer {}", fixture.token()).as_str()),
        "the read went out unauthenticated"
    );
}

/// An answer that declares no length has nothing to disagree with, and is
/// unchanged by everything this area added. A store that streams its bodies
/// is not a store that is lying about them, and a length check that started
/// refusing those would have taken working reads down with it.
#[test]
fn an_answer_that_declares_no_length_is_delivered_whole() {
    let fixture = Fixture::new();
    let body = document();
    let gateway = Gateway::start(Shape::Unlengthed, body.clone());

    let out = fixture.stado(&gateway, &["storage", "cat", KEY]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "an unlengthed answer is complete when the connection closes: {}",
        stderr(&out)
    );
    assert_eq!(out.stdout, body);
    assert!(
        stdout(&out).parse::<serde_json::Value>().is_ok(),
        "the document the reader delivered does not parse: {}",
        stdout(&out)
    );
}
