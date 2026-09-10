//! The answers a resolution gives when the directory and the host disagree.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use crate::fixture::{children_named, http_get, wait_listening, Policy, Serving};
use crate::resolution::{CONCURRENT_READS, GENERATION};
use crate::{Host, TARGET};

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
