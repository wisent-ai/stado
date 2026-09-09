//! A real object API on loopback: one socket, bound by the test, answering
//! the product's own requests.
//!
//! It is not a stand-in for the store — it IS the transport the verdict is
//! computed from. The product opens a TCP connection, writes its request, and
//! reads whatever comes back; what a case chooses is only what this end does
//! with it.
//!
//! Two routes matter. `JobStorage::new` reads the store's layout marker
//! (`system/storage-layout.json`) before anything else, and refuses a store
//! whose marker is missing or belongs to another product, so that request is
//! always answered with the marker the product itself writes. Every other
//! request is the object under test, and gets the case's chosen answer: an
//! HTTP status, or a hang-up with no answer at all.
//!
//! Every request target the socket receives is recorded, so a case can prove
//! something the sentences cannot: that no request left the process.

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// The one object every store construction reads first. Its document is the
/// marker `JobStorage::ensure_layout` writes and validates: product `stado` at
/// the layout schema version this build understands.
const LAYOUT_KEY: &str = "storage-layout.json";
const LAYOUT_DOCUMENT: &str = r#"{"product":"stado","schema_version":1}"#;
/// The marker read is always served, so the store under test constructs and
/// the case measures the object read rather than the store's own preflight.
const LAYOUT_STATUS: u16 = 200;

/// What this end does with the request for the object under test.
#[derive(Clone, Copy)]
pub enum Answer {
    /// Answer with this HTTP status and a short JSON body.
    Status(u16),
    /// Read the request and close the connection without answering, which is
    /// what a store behind a transport that dies mid-read does.
    HangUp,
}

pub struct ObjectApi {
    port: u16,
    seen: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
}

impl ObjectApi {
    pub fn answering(answer: Answer) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback socket of our own");
        let port = listener
            .local_addr()
            .expect("the kernel reports the bound address")
            .port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let recorder = Arc::clone(&seen);
        let flag = Arc::clone(&stop);
        std::thread::spawn(move || {
            for accepted in listener.incoming() {
                if flag.load(Ordering::Relaxed) {
                    return;
                }
                let Ok(mut stream) = accepted else { return };
                let Some(target) = request_target(&mut stream) else {
                    continue;
                };
                record(&recorder, &target);
                if target.contains(LAYOUT_KEY) {
                    respond(&mut stream, LAYOUT_STATUS, LAYOUT_DOCUMENT);
                    continue;
                }
                match answer {
                    Answer::Status(status) => respond(
                        &mut stream,
                        status,
                        r#"{"error":"this store is not serving reads right now"}"#,
                    ),
                    Answer::HangUp => {
                        let _ = stream.shutdown(Shutdown::Both);
                    }
                }
            }
        });
        Self { port, seen, stop }
    }

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Every request target this socket received, in order.
    pub fn requests(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// The requests for the object under test — the layout read the store
    /// always performs is not one of them.
    pub fn object_requests(&self) -> Vec<String> {
        self.requests()
            .into_iter()
            .filter(|target| !target.contains(LAYOUT_KEY))
            .collect()
    }
}

impl Drop for ObjectApi {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Unblock the accept loop so the thread observes the flag and leaves.
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

fn record(seen: &Arc<Mutex<Vec<String>>>, target: &str) {
    seen.lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(target.to_string());
}

/// The request target off the request line, once the whole head has arrived.
///
/// Reading the head to its end matters for the hang-up case: a socket that
/// closes before the client has finished writing produces a different failure
/// from one that closes after the request was fully sent.
fn request_target(stream: &mut TcpStream) -> Option<String> {
    let mut head = Vec::new();
    let mut byte = [0_u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte) {
            Ok(0) | Err(_) => break,
            Ok(_) => head.push(byte[0]),
        }
    }
    let text = String::from_utf8_lossy(&head).into_owned();
    text.lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(usize::from(true)))
        .map(str::to_string)
}

fn respond(stream: &mut TcpStream, status: u16, body: &str) {
    let head = format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n",
        reason(status),
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body.as_bytes());
    let _ = stream.flush();
}

/// The reason phrases for the statuses these cases answer with. The product
/// reads the status code and never the phrase, but a client is entitled to a
/// well-formed status line.
fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        401 => "Unauthorized",
        503 => "Service Unavailable",
        _ => "Status",
    }
}
