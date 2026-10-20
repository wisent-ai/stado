//! The resolver records, per service, when it last answered a relayed
//! request and since when its requests have ended unanswered, in
//! `~/.stado/resolver-answers.json`, so a service whose clients all give up
//! before it answers is told from one that answers.
//!
//! The deployment (`../shed/deployment.rs`) places `elsewhere-api` on this
//! machine at a listener this test holds; the refresh interval and the
//! descriptor limit are the test's own inputs (`STADO_TEST_REFRESH_SECONDS`,
//! `STADO_TEST_FD_LIMIT`). Three requests go through the real `stado serve
//! --resolver`: the service answers the first, the second's client closes
//! before the service answers, and the service answers the third. Expected:
//! after the first, `last_answered_at` and no unanswered record; after the
//! second, `first_unanswered_at` and `last_unanswered_at` beside the kept
//! `last_answered_at`; after the third, both unanswered fields cleared.
#[path = "../shed/deployment.rs"]
mod deployment;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};

use deployment::{Deployment, SERVICE};
use serde_json::Value;

const REQUEST: &[u8] = b"GET / HTTP/1.1\r\nHost: elsewhere\r\n\r\n";
const ANSWER: &[u8] = b"HTTP/1.1 204 No Content\r\n\r\n";

/// A test input, refused by name when the environment does not state it.
fn input(name: &str) -> u64 {
    let value = match std::env::var(name) {
        Ok(value) => value,
        Err(error) => panic!("{name} states this journey's input and is not set: {error}"),
    };
    match value.parse() {
        Ok(value) => value,
        Err(error) => panic!("{name}={value:?} is not a whole number: {error}"),
    }
}

/// The resolver's published record for the service, read until `settled`
/// holds of it: each read is the product's own file, never a guessed wait.
fn record_until(deployment: &Deployment, settled: impl Fn(&Value) -> bool) -> Value {
    let path = deployment.root.join(".stado").join("resolver-answers.json");
    loop {
        let body = match std::fs::read(&path) {
            Ok(body) => body,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => panic!("{} cannot be read: {error}", path.display()),
        };
        let record: Value = match serde_json::from_slice(&body) {
            Ok(record) => record,
            Err(error) => panic!("{} is not JSON: {error}", path.display()),
        };
        if settled(&record[SERVICE]) {
            return record[SERVICE].clone();
        }
    }
}

/// One request through the adapter whose answer is read to its first line.
fn answered_request(adapter_bind: &str) -> String {
    let mut client = TcpStream::connect(adapter_bind).expect("the adapter accepts");
    client.write_all(REQUEST).expect("the request is relayed");
    let mut line = String::new();
    BufReader::new(&client)
        .read_line(&mut line)
        .expect("the answer reads");
    line
}

#[test]
fn each_ended_request_is_recorded_as_answered_or_not() {
    let refresh_seconds = input("STADO_TEST_REFRESH_SECONDS");
    let fd_limit = input("STADO_TEST_FD_LIMIT");
    let mut deployment = Deployment::start();
    deployment.cli(&["config", "init"]);
    let api_bind = deployment.free_bind();
    let adapter_bind = deployment.free_bind();
    let service_bind = deployment.free_bind();
    deployment.import_registry(&api_bind, &adapter_bind, &service_bind, refresh_seconds);

    // The service answers the first and third connection and never the
    // second, which it holds until its client is gone.
    let service = TcpListener::bind(&service_bind).expect("hold the service's bind");
    let served = std::thread::spawn(move || {
        for (stream, answers) in service.incoming().zip([true, false, true]) {
            let mut stream = stream.expect("the service accepts the relay");
            let mut line = String::new();
            BufReader::new(&stream)
                .read_line(&mut line)
                .expect("the relayed request reads");
            if answers {
                stream.write_all(ANSWER).expect("the service answers");
            } else {
                let mut rest = Vec::new();
                stream
                    .read_to_end(&mut rest)
                    .expect("the abandoned relay closes");
            }
        }
    });
    let log = deployment.serve(fd_limit);

    let first = answered_request(&adapter_bind);
    assert!(
        first.starts_with("HTTP/1.1 204"),
        "the first request was not answered: {first:?}"
    );
    let after_answer = record_until(&deployment, |record| record["last_answered_at"].is_string());
    assert!(
        after_answer.get("first_unanswered_at").is_none(),
        "{after_answer}"
    );

    let mut abandoned = TcpStream::connect(&adapter_bind).expect("the adapter accepts");
    abandoned
        .write_all(REQUEST)
        .expect("the request is relayed");
    drop(abandoned);
    let after_abandon = record_until(&deployment, |record| {
        record["first_unanswered_at"].is_string()
    });
    assert_eq!(
        after_abandon["last_answered_at"], after_answer["last_answered_at"],
        "{after_abandon}"
    );
    assert!(
        after_abandon["last_unanswered_at"].is_string(),
        "{after_abandon}"
    );

    let third = answered_request(&adapter_bind);
    assert!(
        third.starts_with("HTTP/1.1 204"),
        "the third request was not answered: {third:?}"
    );
    let after_recovery = record_until(&deployment, |record| {
        record["last_answered_at"] != after_answer["last_answered_at"]
    });
    assert!(
        after_recovery.get("first_unanswered_at").is_none(),
        "{after_recovery}"
    );
    assert!(
        after_recovery.get("last_unanswered_at").is_none(),
        "{after_recovery}"
    );

    std::fs::write(
        deployment.root.join("answers-record.json"),
        after_recovery.to_string(),
    )
    .unwrap();
    served.join().expect("the service saw all three relays");
    drop(deployment);
    let said = log.join().unwrap().join("\n");
    assert!(
        said.contains("abandoned by its client before an answer"),
        "{said}"
    );
}
