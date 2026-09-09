//! A real object gateway, bound on loopback by the case that uses it.
//!
//! The area is about the verdict the product reports for an object request
//! that is not authorized, so the refusal has to arrive the way a refusal
//! arrives: over a socket, from a server that answered. This is that server —
//! about sixty lines of blocking HTTP on `127.0.0.1:0`, with the port chosen
//! by the kernel so cases can run in parallel.
//!
//! It serves the storage-layout marker, because `JobStorage::new` reads that
//! object before anything else and a store that cannot be constructed never
//! reaches the verdict under test. Every other object route answers the status
//! the case declared. That is exactly the shape of a namespace-scoped grant: a
//! reader admitted to one object and refused another.
//!
//! [`Gateway`] is dropped at the end of every case, and dropping it stops the
//! listener and joins its thread, so nothing is left listening afterwards.

use std::io::{BufRead, BufReader, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

/// The object read route and the list route the client addresses, and the one
/// object it must be allowed to read for the store to exist at all. All three
/// are the product's own spellings (`queue::stado_object::uri`,
/// `queue::storage::facade::layout`).
pub const OBJECT_ROUTE: &str = "/api/object";
pub const LIST_ROUTE: &str = "/api/object/list";
const LAYOUT_MARKER: &str = "storage-layout.json";

/// The layout document the marker route answers with: the product name and
/// the layout schema version this build accepts (`queue::STORAGE_LAYOUT_VERSION`).
const LAYOUT_PRODUCT: &str = "stado";
const LAYOUT_SCHEMA_VERSION: u32 = 1;

/// The body a gateway sends with a refusal, copied from the sentence Skarbiec
/// answers a consumer that has no grant on an item.
pub const REFUSAL_BODY: &str = r#"{"error":"consumer not authorized to read item field"}"#;

/// A gateway listening on loopback, answering `status` for every object route
/// except the layout marker.
pub struct Gateway {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    object_requests: Arc<AtomicUsize>,
    server: Option<JoinHandle<()>>,
}

impl Gateway {
    /// Bind and serve until dropped.
    pub fn answering(status: u16) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind the fixture gateway");
        let address = listener.local_addr().expect("read the gateway's own port");
        let stop = Arc::new(AtomicBool::new(false));
        let object_requests = Arc::new(AtomicUsize::new(0));
        let served_stop = Arc::clone(&stop);
        let served_requests = Arc::clone(&object_requests);
        let server = std::thread::spawn(move || {
            for connection in listener.incoming() {
                if served_stop.load(Ordering::SeqCst) {
                    return;
                }
                let Ok(stream) = connection else { return };
                serve(stream, status, &served_requests);
            }
        });
        Self {
            address,
            stop,
            object_requests,
            server: Some(server),
        }
    }

    /// The origin to declare as the store.
    pub fn url(&self) -> String {
        format!("http://{}", self.address)
    }

    /// How many times the object or list route was asked for something other
    /// than the layout marker, so a case can prove the refusal it read came
    /// from a request that was really made.
    pub fn object_requests(&self) -> usize {
        self.object_requests.load(Ordering::SeqCst)
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Unblock the accept loop so the thread observes the flag, then wait
        // for it: a case must not leave a listener behind.
        if let Ok(stream) = TcpStream::connect(self.address) {
            let _ = stream.shutdown(Shutdown::Both);
        }
        if let Some(server) = self.server.take() {
            let _ = server.join();
        }
    }
}

fn serve(mut stream: TcpStream, status: u16, object_requests: &AtomicUsize) {
    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(clone) => clone,
        Err(_) => return,
    });
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() || request_line.trim().is_empty() {
        return;
    }
    // Drain the headers so the client's write completes before the answer.
    loop {
        let mut header = String::new();
        match reader.read_line(&mut header) {
            Ok(0) => break,
            Ok(_) if header.trim().is_empty() => break,
            Ok(_) => continue,
            Err(_) => return,
        }
    }
    let path = request_line.split_whitespace().nth(1).unwrap_or_default();
    let (status, body) = if path.contains(LAYOUT_MARKER) {
        (
            reqwest::StatusCode::OK.as_u16(),
            format!(r#"{{"product":"{LAYOUT_PRODUCT}","schema_version":{LAYOUT_SCHEMA_VERSION}}}"#),
        )
    } else {
        object_requests.fetch_add(1, Ordering::SeqCst);
        (status, REFUSAL_BODY.to_string())
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n{body}",
        reason = reqwest::StatusCode::from_u16(status)
            .ok()
            .and_then(|code| code.canonical_reason())
            .unwrap_or("Status"),
        length = body.len(),
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
    let _ = stream.shutdown(Shutdown::Write);
}
