//! A real object gateway on a real loopback socket.
//!
//! Nothing here stands in for a service. The listener is a `TcpListener` this
//! process binds on `127.0.0.1:0`, it speaks HTTP/1.1 by hand so that the
//! framing is the subject of the test rather than a library's opinion about
//! it, and the product's own client connects to it over TCP and reads what
//! it sends. A gateway that declares one length and sends another is exactly
//! what this area exists to make visible, and a conforming HTTP server cannot
//! be asked to do it.
//!
//! Two routes are always answered whole, because they are not what any case
//! is about: the storage-layout marker every `JobStorage` construction reads
//! first, and the object listing `storage stat` reads for metadata. The
//! object route and the release route carry the [`Shape`] under test.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// The object key every case reads. The canonical registry is the document
/// whose short read started this: it is handed straight to a JSON parser, so
/// a transfer that stopped early is journalled as a malformed registry.
pub const KEY: &str = "registry.json";

/// The version token the object route answers with, so a versioned read fails
/// on the body and never on a missing header. Copied from a live run of the
/// route against this fleet's store.
pub const VERSION: &str = "5dbf12dc1d9c8797002421829f610a72a0d8d76b3be11fe522246196c795b6a8";

/// The size of the live canonical registry, which is what the route declares
/// for it. A case that declares this and sends less is the shape an operator
/// actually hit, and the two numbers in the refusal are the two numbers they
/// would read.
pub const REGISTRY_BYTES: usize = 41_041;

/// The document served whole: a registry as the product writes one.
pub fn document() -> Vec<u8> {
    br#"{"schema_version": 2, "targets": [], "coordinators": []}"#.to_vec()
}

/// How the gateway frames one answer on the route under test.
#[derive(Clone, Copy, Debug)]
pub enum Shape {
    /// `Content-Length` is the body's own length, and the body follows it.
    /// The answer a working store gives.
    Whole,
    /// `Content-Length` declares `declared`, and the body is then framed with
    /// `Transfer-Encoding: chunked` and terminated properly.
    ///
    /// This is the shape a length check owns. The message ends cleanly, so
    /// the client's own framing has nothing to complain about and the body
    /// arrives as a complete read; only comparing it against the declaration
    /// can tell that these are not the object's bytes. It carries a body
    /// shorter than `declared` in one case and longer in another — the
    /// direction is the body's, not the frame's.
    Chunked { declared: usize },
    /// `Content-Length` declares `declared`, the body is sent plain, and the
    /// socket closes with fewer bytes written. The shape a box that fell off
    /// the network mid-transfer leaves.
    ClosedEarly { declared: usize },
    /// No `Content-Length` at all: the length is the connection close, which
    /// is what a streaming route does. There is nothing to disagree with.
    Unlengthed,
}

/// One request this gateway received, as it received it.
#[derive(Clone, Debug)]
pub struct Asked {
    /// The request target, still percent-encoded, exactly as it arrived.
    pub target: String,
    /// The `Authorization` header, if the client sent one.
    pub authorization: Option<String>,
}

/// A gateway that keeps answering until it is dropped.
pub struct Gateway {
    origin: String,
    asked: Arc<Mutex<Vec<Asked>>>,
    running: Arc<AtomicBool>,
}

impl Gateway {
    /// Bind a loopback port and answer the object and release routes with
    /// `shape`, serving `body` as the payload.
    pub fn start(shape: Shape, body: Vec<u8>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port is bindable");
        let origin = format!("http://{}", listener.local_addr().expect("a bound address"));
        listener
            .set_nonblocking(true)
            .expect("the listener accepts a non-blocking mode");
        let asked = Arc::new(Mutex::new(Vec::new()));
        let running = Arc::new(AtomicBool::new(true));
        let thread_asked = Arc::clone(&asked);
        let thread_running = Arc::clone(&running);
        std::thread::spawn(move || {
            while thread_running.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        stream
                            .set_nonblocking(false)
                            .expect("an accepted socket returns to blocking mode");
                        serve(stream, shape, &body, &thread_asked);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Err(_) => return,
                }
            }
        });
        Self {
            origin,
            asked,
            running,
        }
    }

    /// The origin to declare as the store, for example
    /// `http://127.0.0.1:53107`.
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// Every request this socket received, in arrival order. This is the
    /// state the product's read left behind on the far side, and it is what
    /// proves the bytes a case asserts came off this socket rather than out
    /// of a cache, a file or another store.
    pub fn asked(&self) -> Vec<Asked> {
        self.asked
            .lock()
            .expect("the request log is intact")
            .clone()
    }

    /// The requests for the object under test, with the bookkeeping routes
    /// every `JobStorage` construction makes dropped.
    pub fn object_reads(&self) -> Vec<Asked> {
        self.asked()
            .into_iter()
            .filter(|request| !request.target.contains("storage-layout"))
            .filter(|request| !request.target.starts_with("/api/object/list"))
            .collect()
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
    }
}

/// Read one request head off the socket, or `None` if the peer went away.
fn read_head(stream: &mut TcpStream) -> Option<String> {
    let mut raw = Vec::new();
    let mut byte = [0_u8; 1];
    while !raw.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte) {
            Ok(0) | Err(_) => return None,
            Ok(_) => raw.push(byte[0]),
        }
    }
    Some(String::from_utf8_lossy(&raw).into_owned())
}

fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(field, _)| field.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim())
}

fn response_head(extra: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
         Accept-Ranges: bytes\r\nX-Stado-Version: {VERSION}\r\n{extra}\
         Connection: close\r\n\r\n"
    )
}

/// Answer one connection and close it.
fn serve(mut stream: TcpStream, shape: Shape, body: &[u8], asked: &Arc<Mutex<Vec<Asked>>>) {
    let Some(head) = read_head(&mut stream) else {
        return;
    };
    let target = head
        .lines()
        .next()
        .and_then(|line| line.split(' ').nth(1))
        .unwrap_or_default()
        .to_string();
    asked
        .lock()
        .expect("the request log is intact")
        .push(Asked {
            target: target.clone(),
            authorization: header(&head, "authorization").map(str::to_string),
        });

    // The layout marker proves the store belongs to this product, and the
    // listing carries an object's metadata. Neither is what a case is about,
    // so both are always answered whole and correctly.
    if target.contains("storage-layout") {
        let marker = format!(
            r#"{{"product": "stado", "schema_version": {}}}"#,
            stado::queue::STORAGE_LAYOUT_VERSION
        );
        write_whole(&mut stream, marker.as_bytes());
        return;
    }
    if target.starts_with("/api/object/list") {
        write_whole(&mut stream, br#"{"objects": []}"#);
        return;
    }

    match shape {
        Shape::Whole => write_whole(&mut stream, body),
        Shape::Unlengthed => {
            let _ = stream.write_all(response_head("").as_bytes());
            let _ = stream.write_all(body);
        }
        Shape::ClosedEarly { declared } => {
            let _ = stream
                .write_all(response_head(&format!("Content-Length: {declared}\r\n")).as_bytes());
            let _ = stream.write_all(body);
        }
        Shape::Chunked { declared } => {
            let _ = stream.write_all(
                response_head(&format!(
                    "Content-Length: {declared}\r\nTransfer-Encoding: chunked\r\n"
                ))
                .as_bytes(),
            );
            let _ = stream.write_all(format!("{:x}\r\n", body.len()).as_bytes());
            let _ = stream.write_all(body);
            let _ = stream.write_all(b"\r\n0\r\n\r\n");
        }
    }
    let _ = stream.flush();
    // Closing here is what makes the declaration answerable: the gateway says
    // it has finished while its own count disagrees.
    drop(stream);
}

fn write_whole(stream: &mut TcpStream, body: &[u8]) {
    let _ =
        stream.write_all(response_head(&format!("Content-Length: {}\r\n", body.len())).as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}
