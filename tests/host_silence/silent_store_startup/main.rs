//! A resolver whose registry store does not answer while it starts serves
//! the last-known-good registry once the refresh interval that copy declares
//! has passed, instead of standing in `starting` for as long as the store is
//! silent. The store on this host is reached through adapters this very
//! resolver relays, and those relay nothing before the first registry read
//! ends, so a read without a bound held every consumer on the host.
//!
//! The real `stado serve --resolver --target marker-host` runs with an
//! isolated `HOME` under the checkout's build directory (`../refusal_in_flight/run.rs`),
//! a last-known-good registry naming this machine as `marker-host` with one
//! adapter for `marker-api`, and a fleet store this test owns that accepts
//! every connection from the start and answers none. The resolver must say
//! that the store did not answer within the declared refresh interval, bind
//! and serve, and relay a client through the adapter to the answering
//! service. A resolver that never serves leaves this test waiting on its log,
//! which is the defect this test reproduces.
// This journey starts the resolver against a store that never answers; the
// store waits the in-flight refusal journey reads through the run are unused.
#[path = "../refusal_in_flight/run.rs"]
#[allow(dead_code)]
mod run;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::Ordering;

use serde_json::json;

use run::{Run, ANSWER, SERVICE, TARGET};

const BOUNDED: &str = "this host's registry store did not answer within the ";

#[test]
fn a_silent_store_at_startup_is_served_past_from_the_last_known_good_copy() {
    let mut run = Run::new();
    let store = run.store();
    store.silent.store(true, Ordering::SeqCst);
    let service = run.answering_service();
    let api_bind = run.free_bind();
    let adapter_bind = run.free_bind();
    run.write_last_good(&api_bind, &adapter_bind, &service);

    let log = run.serve(&store.address);
    let said = run.report["resolver_log"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|line| line.as_str())
        .find(|line| line.contains(BOUNDED))
        .map(str::to_owned);
    let Some(said) = said else {
        run.save();
        panic!(
            "the resolver served without saying the store did not answer within the declared interval: {}",
            run.report["resolver_log"]
        );
    };
    assert!(
        said.contains(&format!(
            "the last-known-good registry declares for {TARGET}"
        )),
        "the bound is not the interval the last-known-good copy declares: {said}"
    );
    assert!(
        store.held.try_iter().next().is_some(),
        "the resolver served without ever asking the store: {}",
        run.report["resolver_log"]
    );

    let mut stream = TcpStream::connect(&adapter_bind).unwrap();
    write!(stream, "GET /through HTTP/1.1\r\nHost: {SERVICE}\r\n\r\n").unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).unwrap();
    for line in log.try_iter() {
        run.log(line);
    }
    run.report["relayed"] = json!(answer);
    run.save();
    assert!(
        answer.contains(ANSWER),
        "a client is not relayed through the adapter after the bounded startup read: {answer}"
    );
    run.report["outcome"] = json!("passed");
    run.save();
}
