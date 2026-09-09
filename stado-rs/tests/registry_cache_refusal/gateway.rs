//! The fleet store, on a real loopback socket, whose answer a case switches
//! while the product is running.
//!
//! The cache this area defends exists for one situation: the authority is on
//! the other side of a network that has stopped answering. Since `c637b026`
//! the product says so in code — a store that is a directory on this disk
//! neither records nor reads the last-known-good copy, because a path that is
//! gone takes the copy beside it, and a scratch lease writing its one-target
//! document into that cache cost the operator fifty-seven declared hosts. So
//! these cases cannot drive a local store: the recording path is not theirs.
//!
//! Nothing here stands in for a service. The listener is a `TcpListener` this
//! process binds on `127.0.0.1:0`, the product's own object client connects to
//! it over TCP, and the three answers below are the three an object API really
//! gives: the document with its generation, `404` for an object nobody has
//! published, and `503` for a store that cannot answer right now. A removed
//! file cannot express the third, which is exactly the state the copy is for.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// The object key the canonical registry lives under.
pub const KEY: &str = "registry.json";

/// What this gateway answers for [`KEY`].
#[derive(Clone, Debug)]
pub enum Answer {
    /// The document, served whole, with a generation the product treats as an
    /// opaque token and records in the copy's sidecar.
    Serves {
        document: String,
        generation: String,
    },
    /// Nobody has published a registry: a fresh fleet.
    Absent,
    /// The store is up but cannot answer, which is the state the recorded copy
    /// exists to survive.
    Unavailable,
}

/// A gateway that keeps answering until it is dropped.
pub struct Gateway {
    origin: String,
    answer: Arc<Mutex<Answer>>,
    generations: Arc<AtomicUsize>,
    running: Arc<AtomicBool>,
}

impl Gateway {
    /// Bind a loopback port and answer with [`Answer::Absent`] until a case
    /// publishes something.
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port is bindable");
        let origin = format!("http://{}", listener.local_addr().expect("a bound address"));
        listener
            .set_nonblocking(true)
            .expect("the listener accepts a non-blocking mode");
        let answer = Arc::new(Mutex::new(Answer::Absent));
        let running = Arc::new(AtomicBool::new(true));
        let thread_answer = Arc::clone(&answer);
        let thread_running = Arc::clone(&running);
        std::thread::spawn(move || {
            while thread_running.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        stream
                            .set_nonblocking(false)
                            .expect("an accepted socket returns to blocking mode");
                        let current = thread_answer.lock().expect("the answer is intact").clone();
                        serve(stream, &current);
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
            answer,
            generations: Arc::new(AtomicUsize::new(0)),
            running,
        }
    }

    /// The origin to declare as the store, for example
    /// `http://127.0.0.1:53107`.
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// Serve `document` under a generation this store has not used before, the
    /// way an authority that accepted a write reports one.
    pub fn serve(&self, document: &str) -> String {
        let generation = format!(
            "gen-{:04}-{}",
            self.generations.fetch_add(1, Ordering::Relaxed) + 1,
            document.len()
        );
        self.set(Answer::Serves {
            document: document.to_string(),
            generation: generation.clone(),
        });
        generation
    }

    pub fn set(&self, answer: Answer) {
        *self.answer.lock().expect("the answer is intact") = answer;
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

/// Answer one connection and close it.
fn serve(mut stream: TcpStream, answer: &Answer) {
    let Some(head) = read_head(&mut stream) else {
        return;
    };
    let target = head
        .lines()
        .next()
        .and_then(|line| line.split(' ').nth(1))
        .unwrap_or_default()
        .to_string();

    // The layout marker proves the store belongs to this product and the
    // listing carries an object's metadata. Neither is what a case is about,
    // so both are always answered whole and correctly.
    if target.contains("storage-layout") {
        let marker = format!(
            r#"{{"product": "stado", "schema_version": {}}}"#,
            stado::queue::STORAGE_LAYOUT_VERSION
        );
        write_body(&mut stream, "200 OK", None, marker.as_bytes());
        return;
    }
    if target.starts_with("/api/object/list") {
        write_body(&mut stream, "200 OK", None, br#"{"objects": []}"#);
        return;
    }
    if !target.contains(KEY) {
        write_body(&mut stream, "404 Not Found", None, b"");
        return;
    }

    match answer {
        Answer::Serves {
            document,
            generation,
        } => write_body(
            &mut stream,
            "200 OK",
            Some(generation.as_str()),
            document.as_bytes(),
        ),
        Answer::Absent => write_body(&mut stream, "404 Not Found", None, b""),
        Answer::Unavailable => write_body(
            &mut stream,
            "503 Service Unavailable",
            None,
            br#"{"error": "the store cannot answer this question right now"}"#,
        ),
    }
}

fn write_body(stream: &mut TcpStream, status: &str, version: Option<&str>, body: &[u8]) {
    let version = version
        .map(|value| format!("X-Stado-Version: {value}\r\n"))
        .unwrap_or_default();
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\n{version}Connection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}
