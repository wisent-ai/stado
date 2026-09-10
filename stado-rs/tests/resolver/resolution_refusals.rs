//! The running resolver answering exactly one declared consumer and
//! refusing every other read, and the adapter that carries the object API's
//! own answer without paying a process per read.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;


use crate::fixture::{children_named, http_get, wait_listening, wait_published, Policy, Serving};
use crate::resolution::{CONCURRENT_READS, GENERATION};
use crate::{said, stderr, Host, CONSUMER, SERVICE, TARGET};

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

#[test]
fn the_adapter_carries_the_object_apis_own_answer_and_costs_no_process_per_read() {
    let policy = Policy::patient(GENERATION);
    let host = Host::new(&policy.document());
    let object_api = Serving::start(
        &host,
        &[
            "dashboard",
            "--bind",
            "127.0.0.1",
            "--port",
            &policy.upstream.to_string(),
        ],
    );
    assert!(
        wait_listening(policy.upstream),
        "the object API never bound its declared endpoint: {}",
        object_api.said()
    );
    let mut resolver = Serving::start(&host, &["resolver", "serve", "--target", TARGET]);
    assert!(
        wait_listening(policy.adapter),
        "the resolver never bound its declared adapter: {}",
        resolver.said()
    );

    // Both answers are the object API's own: `/healthz` is served with no
    // credential, and an object read from a store with no credential broker
    // configured is refused by the API itself. Carrying a refusal verbatim is
    // as much the contract as carrying a body.
    let health = http_get(policy.adapter, "/healthz", &[]).expect("the adapter answers");
    assert!(
        health.starts_with("HTTP/1.1 200 OK"),
        "the object API's status line did not come back: {health}"
    );
    assert!(
        health.contains("\"ok\":true"),
        "the object API's body did not come back: {health}"
    );

    let object = http_get(
        policy.adapter,
        "/api/object?uri=stado%3A%2F%2Fprobierz%2Fcapacity%2Flocal-probe.json",
        &[],
    )
    .expect("the adapter answers an object read");
    assert!(
        object.starts_with("HTTP/1.1 503"),
        "an object read was not carried to the API: {object}"
    );
    assert!(
        object.contains("object authorization unavailable"),
        "the API's own refusal was not carried back: {object}"
    );

    // The resolver holds no child process per read. Sampled with the
    // operating system's own `pgrep` and `ps` while the reads are in flight.
    let peak = Arc::new(AtomicUsize::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let sampler = {
        let (peak, stop, pid) = (peak.clone(), stop.clone(), resolver.pid());
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                peak.fetch_max(children_named(pid, "stado"), Ordering::Relaxed);
            }
        })
    };
    let readers: Vec<_> = (0..CONCURRENT_READS)
        .map(|_| {
            let adapter = policy.adapter;
            std::thread::spawn(move || http_get(adapter, "/healthz", &[]))
        })
        .collect();
    let answers: Vec<_> = readers
        .into_iter()
        .map(|reader| reader.join().expect("a reader thread joins"))
        .collect();
    stop.store(true, Ordering::Relaxed);
    sampler.join().expect("the sampler thread joins");

    for (index, answer) in answers.iter().enumerate() {
        let answer = answer.as_ref().unwrap_or_else(|error| {
            panic!("read {index} of {CONCURRENT_READS} was not answered: {error}")
        });
        assert!(
            answer.starts_with("HTTP/1.1 200 OK"),
            "read {index} of {CONCURRENT_READS} got: {answer}"
        );
    }
    let peak = peak.load(Ordering::Relaxed);
    assert_eq!(
        peak,
        0,
        "the resolver spawned {peak} child process(es) for {CONCURRENT_READS} reads to a service \
         on this host: {}",
        resolver.said()
    );
    assert!(
        resolver.running(),
        "the resolver died under {CONCURRENT_READS} reads: {}",
        resolver.said()
    );
}
