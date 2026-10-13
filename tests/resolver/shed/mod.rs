//! A resolver whose listener runs out of file descriptors sheds the relays
//! it holds unanswered past its refresh interval and accepts again, instead
//! of ending the adapter and standing `failed` with every held socket open.
//!
//! The deployment (`deployment.rs`) places `elsewhere-api` on this machine
//! at a listener this test holds and never answers on; the refresh interval
//! and the descriptor limit are the test's own inputs
//! (`STADO_TEST_REFRESH_SECONDS`, `STADO_TEST_FD_LIMIT`). A first wave of
//! clients each sends a request the service never answers; `stado resolver
//! status` is asked until it reports them held past the interval; a second
//! wave then drives the listener out of descriptors. Expected: the resolver
//! says it shed the held relays, each first-wave client reads the resolver's
//! refusal naming the descriptors and the interval, a client connecting
//! afterwards is accepted, and the published state is not `failed`.
mod deployment;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

use deployment::Deployment;

const REQUEST: &[u8] = b"GET / HTTP/1.1\r\nHost: elsewhere\r\n\r\n";

/// A test input, refused by name when the environment does not state it.
fn input(name: &str) -> u64 {
    let value = match std::env::var(name) {
        Ok(value) => value,
        Err(error) => panic!("{name} is required in the environment: {error}"),
    };
    match value.parse() {
        Ok(count) => count,
        Err(error) => panic!("{name}={value:?} is not a count: {error}"),
    }
}

/// Up to `count` clients, each connected to the adapter with a request sent;
/// stops at the first connection the adapter does not take.
fn wave(adapter_bind: &str, count: u64) -> Vec<TcpStream> {
    let mut clients = Vec::new();
    for _ in std::iter::repeat(()).take(count as usize) {
        let Ok(mut client) = TcpStream::connect(adapter_bind) else {
            break;
        };
        if client.write_all(REQUEST).is_err() {
            break;
        }
        clients.push(client);
    }
    clients
}

#[test]
fn held_relays_are_shed_when_the_listener_runs_out_of_descriptors() {
    let refresh_seconds = input("STADO_TEST_REFRESH_SECONDS");
    let fd_limit = input("STADO_TEST_FD_LIMIT");
    let mut deployment = Deployment::start();
    deployment.cli(&["config", "init"]);
    let api_bind = deployment.free_bind();
    let adapter_bind = deployment.free_bind();
    let service_bind = deployment.free_bind();
    deployment.import_registry(&api_bind, &adapter_bind, &service_bind, refresh_seconds);

    // The service: accepts every connection and answers none of them.
    let service = TcpListener::bind(&service_bind).expect("hold the service's bind");
    let accepted = std::thread::spawn(move || {
        let mut held = Vec::new();
        for stream in service.incoming() {
            match stream {
                Ok(stream) => held.push(stream),
                Err(_) => break,
            }
        }
        held
    });
    let log = deployment.serve(fd_limit);

    // The first wave: each client sends a request and waits on it, until the
    // resolver's own status reports them held past the interval.
    let first_wave = wave(&adapter_bind, fd_limit);
    assert!(
        !first_wave.is_empty(),
        "no client could connect to the adapter at {adapter_bind}"
    );
    let held = deployment.until_held(&adapter_bind);
    std::fs::write(deployment.root.join("held-status.json"), &held).unwrap();

    // The second wave: more connections than the limit leaves, until accept
    // fails for want of descriptors and the resolver sheds the first wave.
    let second_wave = wave(&adapter_bind, fd_limit);

    // Every shed client reads the resolver's refusal, to its close, naming
    // the cause; a client the shed did not reach would read forever, which
    // is the failure this test is for.
    let mut refusals = Vec::new();
    for mut client in first_wave {
        let mut answer = String::new();
        client
            .read_to_string(&mut answer)
            .expect("the shed client's refusal reads to its close");
        refusals.push(answer);
    }
    let refusal = refusals.first().expect("the first wave had a client");
    assert!(
        refusal.contains("ran out of file descriptors")
            && refusal.contains(&format!("{refresh_seconds}s refresh interval")),
        "the refusal names the descriptors and the interval: {refusal}"
    );

    // The adapter accepts again, and the resolver did not stand failed.
    drop(second_wave);
    let mut again = TcpStream::connect(&adapter_bind).expect("the adapter accepts after shedding");
    again
        .write_all(REQUEST)
        .expect("a request after shedding is relayed");
    let state = deployment.published_state();
    assert_ne!(
        state["state"], "failed",
        "the resolver stands failed after shedding: {state}"
    );
    drop(again);
    drop(deployment);
    let said = log.join().unwrap().join("\n");
    assert!(
        said.contains("shed") && said.contains("accepts again"),
        "the resolver's log says what it shed: {said}"
    );
    drop(accepted);
}
