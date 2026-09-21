//! Resolving a declared service on this machine, end to end.
//!
//! The declared endpoint travels three ways here and each one is checked
//! against something the product left behind: into the CLI's report, into the
//! forward marker on disk, and — with a real `resolver serve` in front of the
//! real object API — into the HTTP answer a consumer reads on its own
//! loopback port. The active host is this machine, so the adapter takes its
//! local-upstream path and no connection to any other host is opened.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::json;

use crate::fixture::{http_get, wait_listening, wait_published, Policy, Serving};
use crate::{report, said, stderr, Host, CONSUMER, SERVICE, TARGET};

/// The generation this area's authority publishes.
pub(crate) const GENERATION: u64 = 7;
/// Reads issued at once. Large enough that one process per read would be
/// unmistakable in the child count, which is the shape that walked the
/// resolver into its own descriptor budget.
pub(crate) const CONCURRENT_READS: usize = 24;

#[test]
fn a_declared_service_resolves_to_its_endpoint_and_the_marker_lands_on_disk() {
    let policy = Policy::patient(GENERATION);
    let host = Host::new(&policy.document());
    let endpoint = format!("http://127.0.0.1:{}", policy.upstream);

    let answer = host.stado(&[
        "resolver",
        "resolve",
        SERVICE,
        "--consumer",
        CONSUMER,
        "--json",
    ]);
    assert!(
        answer.status.success(),
        "resolving a declared service failed: {}",
        said(&answer)
    );
    assert_eq!(
        report(&answer),
        json!({
            "service": format!("stado://service/{SERVICE}"),
            "generation": GENERATION,
            "capabilities": ["object-store"],
        })
    );

    let answer = host.stado(&["route", "list", "--json"]);
    assert!(answer.status.success(), "{}", said(&answer));
    let listed = report(&answer);
    assert_eq!(listed["authority"]["target"], TARGET);
    assert_eq!(listed["services"][0]["service"], SERVICE);
    assert_eq!(listed["services"][0]["active_host"], TARGET);
    assert_eq!(listed["services"][0]["endpoints"][0]["target"], TARGET);
    assert_eq!(listed["services"][0]["endpoints"][0]["url"], endpoint);
    assert_eq!(
        listed["services"][0]["local_forward"],
        serde_json::Value::Null,
        "nothing has been opened yet: {listed}"
    );

    let answer = host.stado(&["route", "open", SERVICE, "--local", "--json"]);
    assert!(answer.status.success(), "{}", said(&answer));
    let opened = report(&answer);
    assert_eq!(opened["status"], "open");
    assert_eq!(opened["endpoint"], endpoint);
    assert_eq!(opened["forward"]["location"], "local");
    assert_eq!(
        opened["forward"]["marker"],
        host.marker(SERVICE).display().to_string()
    );
    // The report is corroboration; this is the state the command left.
    assert_eq!(
        std::fs::read_to_string(host.marker(SERVICE)).expect("the marker was written"),
        format!("{endpoint}\n")
    );

    let answer = host.stado(&["route", "close", SERVICE]);
    assert!(answer.status.success(), "{}", said(&answer));
    assert!(
        !host.marker(SERVICE).exists(),
        "closing the forward left its marker behind"
    );
}

#[test]
fn the_running_resolver_answers_the_declared_consumer_and_refuses_every_other_read() {
    let policy = Policy::patient(GENERATION);
    let host = Host::new(&policy.document());
    let mut resolver = Serving::start(&host, &["resolver", "serve", "--target", TARGET]);
    assert!(
        wait_listening(policy.api),
        "the resolver never bound its declared API: {}",
        resolver.said()
    );
    wait_published(&host, "serving");
    let consumer = [("x-stado-consumer", CONSUMER)];

    let health = http_get(policy.api, "/health", &[]).expect("the resolution API answers");
    assert!(
        health.starts_with("HTTP/1.1 200 OK"),
        "the resolution API is up but unhealthy: {health}"
    );
    assert!(
        health.contains(&format!(
            "{{\"status\":\"ok\",\"service\":\"stado-resolver\",\"generation\":{GENERATION}}}"
        )),
        "got: {health}"
    );

    let resolved = http_get(
        policy.api,
        &format!("/v1/resolve/service/{SERVICE}"),
        &consumer,
    )
    .expect("the resolution API answers an authorized read");
    assert!(resolved.starts_with("HTTP/1.1 200 OK"), "got: {resolved}");
    assert!(
        resolved.contains(&format!(
            "\"gateway_url\":\"http://127.0.0.1:{}\"",
            policy.adapter
        )),
        "the answer does not name the adapter this consumer must use: {resolved}"
    );
    assert!(
        resolved.contains("\"capabilities\":[\"object-store\"]"),
        "got: {resolved}"
    );

    // A read with no consumer identity at all.
    let anonymous = http_get(policy.api, &format!("/v1/resolve/service/{SERVICE}"), &[])
        .expect("a refusal is an answer");
    assert!(
        anonymous.starts_with("HTTP/1.1 401 Unauthorized"),
        "got: {anonymous}"
    );
    assert!(
        anonymous.contains("{\"error\":\"consumer_required\"}"),
        "got: {anonymous}"
    );

    // A consumer the route does not authorize, and a service nothing declares.
    let intruder = http_get(
        policy.api,
        &format!("/v1/resolve/service/{SERVICE}"),
        &[("x-stado-consumer", "intruder")],
    )
    .expect("a refusal is an answer");
    assert!(
        intruder.starts_with("HTTP/1.1 503 Service Unavailable"),
        "got: {intruder}"
    );
    assert!(
        intruder.contains(&format!(
            "consumer \\\"intruder\\\" is not authorized for service \\\"{SERVICE}\\\""
        )),
        "got: {intruder}"
    );

    let unknown = http_get(policy.api, "/v1/resolve/service/no-such-service", &consumer)
        .expect("a refusal is an answer");
    assert!(
        unknown.starts_with("HTTP/1.1 503 Service Unavailable"),
        "got: {unknown}"
    );
    assert!(
        unknown.contains("unknown logical service \\\"no-such-service\\\""),
        "got: {unknown}"
    );

    let elsewhere = http_get(policy.api, "/v1/resolve", &consumer).expect("a refusal is an answer");
    assert!(
        elsewhere.starts_with("HTTP/1.1 404 Not Found"),
        "got: {elsewhere}"
    );
    assert!(resolver.running(), "the resolver died: {}", resolver.said());

    // The same two refusals through the CLI, which resolves against the
    // registry rather than against the running process.
    let answer = host.stado(&[
        "resolver",
        "resolve",
        "no-such-service",
        "--consumer",
        CONSUMER,
    ]);
    assert_eq!(answer.status.code(), Some(1));
    assert!(
        stderr(&answer).contains("Error: unknown logical service \"no-such-service\""),
        "got: {}",
        said(&answer)
    );
    let answer = host.stado(&["resolver", "resolve", SERVICE, "--consumer", "intruder"]);
    assert_eq!(answer.status.code(), Some(1));
    assert!(
        stderr(&answer).contains(&format!(
            "Error: consumer \"intruder\" is not authorized for service \"{SERVICE}\""
        )),
        "got: {}",
        said(&answer)
    );
}

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
