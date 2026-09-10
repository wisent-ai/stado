//! The native interactive workload stream, over a real WebSocket.
//!
//! Two promises `GET /api/operator/workload/attach` publishes are asserted
//! here against the real listener, the real CLI child and the real installed
//! Jeden runtime: an attachment carrying no explicit mutation confirmation is
//! refused, and a socket that goes away ends the workload it attached rather
//! than leaving a Jeden holding the host. The second is the one an operator
//! pays for — a disconnected console must not be indistinguishable from an
//! abandoned process group on a fleet host.
//!
//! See [`area`] for the isolation contract. Nothing here is a stand-in: the
//! listener, the socket, the CLI child and the runtime are all real.

mod area;

use futures::StreamExt;
use serde_json::Value;
use tokio_tungstenite::tungstenite::Message;

use area::{Area, Socket, DEADLINE, POLL};

/// The stream's own channel bytes: standard output, then standard error.
const STDOUT_CHANNEL: u8 = 1;
const STDERR_CHANNEL: u8 = 2;
/// What Jeden's RPC surface prints when it is ready to serve.
const RUNTIME_READY: &str = "jeden-rpc";

/// The next control frame, with every process frame collected on the way.
async fn control_frame(socket: &mut Socket, output: &mut String) -> Value {
    let deadline = tokio::time::Instant::now() + DEADLINE;
    loop {
        let message = tokio::time::timeout_at(deadline, socket.next())
            .await
            .expect("the stream answered before the deadline")
            .expect("the stream stayed open")
            .expect("the frame is readable");
        match message {
            Message::Text(text) => {
                return serde_json::from_str(&text).expect("a control frame is JSON")
            }
            Message::Binary(bytes) => {
                let (channel, payload) = bytes.split_first().expect("a channel byte");
                assert!(
                    matches!(*channel, STDOUT_CHANNEL | STDERR_CHANNEL),
                    "a stream frame carried an unknown channel byte: {channel}"
                );
                output.push_str(&String::from_utf8_lossy(payload));
            }
            other => panic!("the stream sent an unexpected frame: {other:?}"),
        }
    }
}

/// Read process frames until the attached runtime says it is serving.
async fn wait_for_runtime(socket: &mut Socket) -> String {
    let deadline = tokio::time::Instant::now() + DEADLINE;
    let mut output = String::new();
    while !output.contains(RUNTIME_READY) {
        let message = tokio::time::timeout_at(deadline, socket.next())
            .await
            .expect("the attached runtime announced itself before the deadline")
            .expect("the stream stayed open")
            .expect("the frame is readable");
        match message {
            Message::Binary(bytes) => {
                let (channel, payload) = bytes.split_first().expect("a channel byte");
                assert!(
                    matches!(*channel, STDOUT_CHANNEL | STDERR_CHANNEL),
                    "a stream frame carried an unknown channel byte: {channel}"
                );
                output.push_str(&String::from_utf8_lossy(payload));
            }
            Message::Text(text) => panic!("the attachment ended instead of running: {text}"),
            other => panic!("the stream sent an unexpected frame: {other:?}"),
        }
    }
    output
}

#[tokio::test]
async fn an_attachment_without_the_mutation_confirmation_is_refused_and_starts_nothing() {
    let area = Area::start().await;
    let mut socket = area.attach(Area::request("")).await;

    let mut output = String::new();
    let answer = control_frame(&mut socket, &mut output).await;
    area.retain("unconfirmed.json", &answer.to_string());
    assert_eq!(answer["type"], "error", "{answer}");
    assert_eq!(
        answer["message"], "workload attachment requires explicit RUN_MUTATION confirmation",
        "{answer}"
    );
    assert!(output.is_empty(), "a refused attachment printed: {output}");
    assert_eq!(
        area.attached_jeden(),
        Vec::<String>::new(),
        "a refused attachment started a runtime"
    );
    assert_eq!(
        area.ledgers(),
        Vec::<String>::new(),
        "a refused attachment created a session ledger"
    );
}

#[tokio::test]
async fn dropping_the_socket_ends_the_workload_it_attached() {
    let area = Area::start().await;
    let mut socket = area.attach(Area::request("RUN_MUTATION")).await;

    let mut output = String::new();
    let attached = control_frame(&mut socket, &mut output).await;
    assert_eq!(attached["type"], "attached", "{attached}");
    assert_eq!(attached["protocol"], "stado.workload.v1", "{attached}");

    // The runtime's own readiness line proves the real Jeden is serving under
    // this attachment before the socket is taken away.
    let printed = wait_for_runtime(&mut socket).await;
    area.retain("attached.txt", &printed);
    let running = area.attached_jeden();
    area.retain("running.txt", &running.join("\n"));
    assert!(
        !running.is_empty(),
        "the attachment started no runtime under {}: {printed}",
        area.home.display()
    );

    drop(socket);
    let deadline = tokio::time::Instant::now() + DEADLINE;
    loop {
        let remaining = area.attached_jeden();
        if remaining.is_empty() {
            area.retain("ended.txt", &running.join("\n"));
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the dropped socket left {remaining:?} running under {}",
            area.home.display()
        );
        tokio::time::sleep(POLL).await;
    }
}
