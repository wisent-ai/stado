//! The idle window a proxied connection is measured against.
//!
//! The window used to be applied to each direction on its own, and a request
//! and response connection is silent in the request direction for as long as
//! the service answers - so a response still arriving was cut at the window and
//! the client read a closed connection with no sentence naming the proxy. The
//! window is the time since a byte moved in either direction now, and these
//! cases hold it to that: a streamed answer keeps its connection, a service
//! that never answers gets a named gateway timeout, and a connection with
//! nothing moving is still closed with the reason printed.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::fixture::{wait_listening, Policy, Serving};
use crate::resolution::GENERATION;
use crate::Host;

/// The idle window a proxied connection is measured against, short enough to
/// elapse inside a case.
const IMPATIENT_IDLE_SECONDS: u64 = 1;
/// How long the upstream thinks before its first byte: several idle windows,
/// which is what a loaded host looks like from the proxy.
const SLOW_ANSWER: Duration = Duration::from_secs(4);
/// A streamed answer: this many pieces, this far apart. Each gap is shorter
/// than the window and the whole answer is several windows long.
const STREAM_PIECES: usize = 8;
const STREAM_GAP: Duration = Duration::from_millis(400);
/// What a client waits for a connection the resolver has to close by itself.
const CLOSE_BUDGET: Duration = Duration::from_secs(20);
/// The body the upstreams in these cases answer with.
const SLOW_BODY: &str = "the answer that arrived after the window";

/// An answer being streamed is not an idle connection.
///
/// The window was applied to each direction on its own, and a request/response
/// connection is silent in the request direction for as long as the service
/// answers — so a response still arriving was cut at the window, and the
/// client read `connection closed before message completed` with no sentence
/// naming the proxy. The window is now the time since a byte moved in EITHER
/// direction, so a service that keeps writing keeps its connection.
#[test]
fn an_answer_still_arriving_is_not_cut_by_the_idle_window() {
    let policy = Policy::impatient(GENERATION, IMPATIENT_IDLE_SECONDS);
    let host = Host::new(&policy.document());
    let upstream = streaming_upstream(policy.upstream);
    let mut resolver = Serving::start(&host, &["resolver", "serve", "--target", TARGET]);
    assert!(
        wait_listening(policy.adapter),
        "the adapter never opened: {}",
        resolver.said()
    );

    let answer = http_get(policy.adapter, "/api/object", &[]).expect("the proxied answer");
    upstream.join().expect("the upstream thread ends");

    assert!(
        answer.starts_with("HTTP/1.1 200 OK"),
        "a streamed answer did not arrive: {answer:?}\n{}",
        resolver.said()
    );
    assert_eq!(
        answer.matches(SLOW_BODY).count(),
        STREAM_PIECES,
        "the proxy cut an answer that was still arriving: {answer:?}\n{}",
        resolver.said()
    );
    assert!(resolver.running(), "the resolver died: {}", resolver.said());
}

/// A service that says nothing at all for longer than the window loses the
/// connection — and the client is told which service, and which window.
///
/// The bound exists because a resolver that keeps every socket runs out of
/// descriptors and takes the local data plane down with `Too many open
/// files`. What was missing is the sentence: a caller read a transport error
/// that named neither the proxy nor the service.
#[test]
fn a_service_that_never_answers_gets_a_named_gateway_timeout() {
    let policy = Policy::impatient(GENERATION, IMPATIENT_IDLE_SECONDS);
    let host = Host::new(&policy.document());
    let upstream = slow_upstream(policy.upstream, SLOW_ANSWER);
    let mut resolver = Serving::start(&host, &["resolver", "serve", "--target", TARGET]);
    assert!(
        wait_listening(policy.adapter),
        "the adapter never opened: {}",
        resolver.said()
    );

    let answer = http_get(policy.adapter, "/api/object", &[]).expect("an answer, not a bare close");
    upstream.join().expect("the upstream thread ends");

    assert!(
        answer.starts_with("HTTP/1.1 504 Gateway Timeout"),
        "a client whose service never answered got no answer at all: {answer:?}\n{}",
        resolver.said()
    );
    assert!(
        answer.contains(SERVICE) && answer.contains("idle window this adapter declares"),
        "the timeout names neither the service nor the window: {answer:?}"
    );
    assert!(
        wait_until_said(&resolver, "closed an idle connection"),
        "the resolver cut a connection and said nothing about it: {}",
        resolver.said()
    );
    assert!(resolver.running(), "the resolver died: {}", resolver.said());
}

/// The bound that window exists for is unchanged: a connection where nothing
/// moves in either direction is still closed, and the resolver says it closed
/// it.
#[test]
fn a_connection_with_nothing_moving_is_still_closed_and_the_reason_is_printed() {
    let policy = Policy::impatient(GENERATION, IMPATIENT_IDLE_SECONDS);
    let host = Host::new(&policy.document());
    let upstream = silent_upstream(policy.upstream);
    let resolver = Serving::start(&host, &["resolver", "serve", "--target", TARGET]);
    assert!(
        wait_listening(policy.adapter),
        "the adapter never opened: {}",
        resolver.said()
    );

    let mut client = TcpStream::connect(format!("127.0.0.1:{}", policy.adapter))
        .expect("the adapter accepts a connection");
    client
        .set_read_timeout(Some(CLOSE_BUDGET))
        .expect("a bounded read");
    let mut answer = Vec::new();
    let opened = Instant::now();
    client
        .read_to_end(&mut answer)
        .expect("the resolver closes an idle connection rather than holding it");
    let held = opened.elapsed();
    drop(client);
    upstream.join().expect("the upstream thread ends");

    assert!(
        held < CLOSE_BUDGET,
        "an idle connection was held {held:?}, past the {CLOSE_BUDGET:?} a client waits"
    );
    assert!(
        answer.is_empty(),
        "a connection nobody used carried bytes: {answer:?}"
    );
    assert!(
        wait_until_said(&resolver, "closed an idle connection"),
        "the resolver closed a connection and said nothing about it: {}",
        resolver.said()
    );
}

/// Wait, bounded, for the served resolver to print a sentence.
fn wait_until_said(resolver: &Serving, sentence: &str) -> bool {
    let deadline = Instant::now() + CLOSE_BUDGET;
    while Instant::now() < deadline {
        if resolver.said().contains(sentence) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

/// A real HTTP server on the upstream port that reads the request, says
/// nothing at all for `delay`, and only then answers. A proxy measuring
/// silence has nothing to keep the connection for.
fn slow_upstream(port: u16, delay: Duration) -> JoinHandle<()> {
    let listener =
        TcpListener::bind(format!("127.0.0.1:{port}")).expect("the upstream port is free");
    std::thread::spawn(move || {
        let (mut connection, _) = listener.accept().expect("the proxy dials the upstream");
        let mut head = [0_u8; 1024];
        let _ = connection.read(&mut head).expect("the proxied request");
        std::thread::sleep(delay);
        let answer = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{SLOW_BODY}",
            SLOW_BODY.len()
        );
        // The proxy has closed this connection by now; writing into it is how
        // the case learns that, not a failure of the fixture.
        let _ = connection.write_all(answer.as_bytes());
        let _ = connection.flush();
    })
}

/// An upstream that answers in pieces: each gap shorter than the idle window,
/// the whole answer several windows long. What a service streaming a large
/// object looks like to the proxy.
fn streaming_upstream(port: u16) -> JoinHandle<()> {
    let listener =
        TcpListener::bind(format!("127.0.0.1:{port}")).expect("the upstream port is free");
    std::thread::spawn(move || {
        let (mut connection, _) = listener.accept().expect("the proxy dials the upstream");
        let mut head = [0_u8; 1024];
        let _ = connection.read(&mut head).expect("the proxied request");
        let length = SLOW_BODY.len() * STREAM_PIECES;
        connection
            .write_all(
                format!("HTTP/1.1 200 OK\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n")
                    .as_bytes(),
            )
            .expect("the upstream answers");
        connection.flush().expect("the upstream flushes");
        for _ in 0..STREAM_PIECES {
            std::thread::sleep(STREAM_GAP);
            connection
                .write_all(SLOW_BODY.as_bytes())
                .expect("the upstream keeps answering");
            connection.flush().expect("the upstream flushes");
        }
    })
}

/// An upstream that accepts and then says nothing at all, which is the
/// connection the idle window is there to reclaim.
fn silent_upstream(port: u16) -> JoinHandle<()> {
    let listener =
        TcpListener::bind(format!("127.0.0.1:{port}")).expect("the upstream port is free");
    std::thread::spawn(move || {
        let (connection, _) = listener.accept().expect("the proxy dials the upstream");
        std::thread::sleep(CLOSE_BUDGET);
        drop(connection);
    })
}
