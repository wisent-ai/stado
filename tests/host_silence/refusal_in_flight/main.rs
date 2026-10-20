//! A resolver serving a stale service directory records the staleness in
//! the fleet store once, not once per connection it relays, while the store
//! does not answer: a refusal whose record is still being written is not
//! written again, and the fleet store is opened once for all of them.
//!
//! The real `stado serve --resolver --target marker-host` runs with an
//! isolated `HOME` under the test's output directory (`run.rs`), a
//! last-known-good registry naming this machine as `marker-host` and a
//! registry authority nobody can reach (`.invalid`); its max-stale window is
//! the refresh interval the shared template declares, so the directory goes
//! stale soon after start and stays stale. Its fleet store is a real
//! `stado serve --api` verifying its namespace against a real Skarbiec
//! (`store.rs`): it answers while the resolver starts, by its namespace's
//! policy, which grants the resolver no registry read, so the resolver
//! serves the last-known-good copy; then it is stopped with SIGSTOP and
//! accepts every connection and answers none, as a store host that stopped
//! answering. As many clients as this machine runs threads in parallel are
//! then relayed one after another through the adapter for `marker-api`, a
//! second real object API, each resolution found stale and recorded. The
//! resolver's wait register must hold one request to the store, not one per
//! relayed connection.
mod run;

use std::io::{Read, Write};
use std::net::TcpStream;

use serde_json::{json, Value};

use run::{Run, RELAYED_ANSWER, RELAYED_REQUEST, SERVICE};

#[test]
fn a_stale_directory_is_recorded_once_while_the_store_does_not_answer() {
    let clients = std::thread::available_parallelism()
        .expect("this machine reports how many threads it runs in parallel");
    assert!(
        clients > std::num::NonZeroUsize::MIN,
        "one relayed connection cannot tell one record per connection from one record"
    );
    let mut run = Run::new();
    let store = run.store();
    let api_bind = run.free_bind();
    let adapter_bind = run.free_bind();
    run.write_last_good(&api_bind, &adapter_bind, &store.service);

    let log = run.serve(&store);
    store.hang();

    // The directory is stale once the resolver says so, read back from its
    // own status rather than from a clock this test keeps.
    loop {
        let status: Value =
            serde_json::from_str(&run.cli(&["resolver", "status", "--json"])).unwrap();
        if status["stale"] == true {
            run.report["stale_status"] = status;
            break;
        }
    }

    let request = format!("{RELAYED_REQUEST}\r\nHost: {SERVICE}\r\nConnection: close\r\n\r\n");
    for (client, ()) in std::iter::repeat_n((), clients.get()).enumerate() {
        let mut stream = TcpStream::connect(&adapter_bind).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).unwrap();
        assert!(
            answer.starts_with("HTTP/1.1 200") && answer.contains(RELAYED_ANSWER),
            "client {client} is relayed through the adapter to the serving object API while the directory is stale: {answer}"
        );
    }
    // One more command after the last relay, so every recording the relays
    // started has had its turn to open its request.
    run.cli(&["resolver", "status", "--json"]);

    let waits = run.store_waits(&store.address);
    run.report["clients"] = json!(clients.get());
    run.report["store_waits"] = json!(waits);
    for line in log.try_iter() {
        run.log(line);
    }
    run.save();
    let [_] = waits.as_slice() else {
        panic!(
            "the resolver holds one request to the unanswering store for {} stale resolutions, not one per resolution: {waits:#?}",
            clients.get()
        );
    };
    run.report["outcome"] = json!("passed");
    run.save();
}
