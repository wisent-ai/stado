//! The declared resolver policy, and the real processes these cases run.
//!
//! A `Serving` is a product process this area leaves running and ends when
//! the case ends however it ends — a panic must not leak a listener into the
//! next case. Its output is kept rather than discarded: a command that
//! refuses to start says why on stderr, and a case that threw that away could
//! only report whichever socket it happened to be waiting on.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::{
    free_port, hostname, platform, Host, CONSUMER, REGISTRY_SCHEMA_VERSION, SERVICE, TARGET,
};

/// An hour: longer than any case here runs, so a `patient` resolver keeps the
/// snapshot it loaded at startup.
const PATIENT_REFRESH_SECONDS: u64 = 3600;
/// One second: the shortest interval `refresh_seconds: must be positive`
/// allows, and the shortest `max_stale_seconds` the contract lets accompany
/// it (`must not be shorter than refresh_seconds`).
const EAGER_REFRESH_SECONDS: u64 = 1;
/// How long a case waits for a real socket or a real state file before
/// calling it absent. Generous because a debug build starting on a loaded
/// machine is slow, not because anything here is a race.
pub const BUDGET: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_millis(50);

/// The resolver policy one fixture declares: the generation the authority
/// publishes, the three loopback ports, and the refresh window.
pub struct Policy {
    pub generation: u64,
    pub api: u16,
    pub adapter: u16,
    pub upstream: u16,
    pub refresh_seconds: u64,
    pub max_stale_seconds: u64,
}

impl Policy {
    /// A resolver that will not refresh again inside a case: it holds the
    /// generation it started with, which is what makes "the authority moved
    /// and this resolver did not" an observation rather than a race.
    pub fn patient(generation: u64) -> Self {
        Self::new(generation, PATIENT_REFRESH_SECONDS)
    }

    /// A resolver that refreshes every second and calls its snapshot stale
    /// one second after loading it, so a real snapshot can age past the
    /// window its own target declares inside a case.
    pub fn eager(generation: u64) -> Self {
        Self::new(generation, EAGER_REFRESH_SECONDS)
    }

    fn new(generation: u64, seconds: u64) -> Self {
        Self {
            generation,
            api: free_port(),
            adapter: free_port(),
            upstream: free_port(),
            refresh_seconds: seconds,
            max_stale_seconds: seconds,
        }
    }

    /// The whole document: one target, which is this machine, serving as its
    /// own directory authority.
    pub fn document(&self) -> Value {
        self.document_for(&hostname())
    }

    /// The same document declared for another host identity — what asking
    /// about any machine other than this one means.
    pub fn document_for(&self, identity: &str) -> Value {
        json!({
            "schema_version": REGISTRY_SCHEMA_VERSION,
            "targets": [self.target(identity)],
            "coordinators": [],
            "service_directory": {
                "authority": {"target": TARGET, "command": env!("CARGO_BIN_EXE_stado")},
                "generation": self.generation,
                "services": {
                    SERVICE: {
                        "managed_service": SERVICE,
                        "active_host": TARGET,
                        "endpoints": {
                            TARGET: {"url": format!("http://127.0.0.1:{}", self.upstream)},
                        },
                        "consumers": {CONSUMER: {"capabilities": ["object-store"]}},
                    },
                },
            },
        })
    }

    /// One target entry.
    ///
    /// `ssh` names this machine's own loopback because the service-directory
    /// contract requires the authority target to declare a connection path.
    /// It is deliberately a different identity from `hostnames`, which the
    /// contract also requires — one identity claimed twice is refused with
    /// `host identity '...' is already declared by ...` — and it is never
    /// dialled: the authority read for this target is the local store read.
    pub fn target(&self, identity: &str) -> Value {
        json!({
            "name": TARGET,
            "kind": "local",
            "ssh": "nobody@127.0.0.1",
            "release_platform": platform(),
            "hostnames": [identity],
            "services": [{
                "name": SERVICE,
                "unit": "",
                "label": format!("com.wisent.compute.service.{SERVICE}"),
                "path": format!("/Library/LaunchAgents/com.wisent.compute.service.{SERVICE}.plist"),
                "kind": "launchd",
                "managed_since": "2026-08-01T00:00:00+00:00",
            }],
            "service_resolver": {
                "api_bind": format!("127.0.0.1:{}", self.api),
                "refresh_seconds": self.refresh_seconds,
                "max_stale_seconds": self.max_stale_seconds,
                "adapters": [{
                    "service": SERVICE,
                    "consumer": CONSUMER,
                    "bind": format!("127.0.0.1:{}", self.adapter),
                }],
            },
        })
    }
}

pub struct Serving {
    child: Child,
    output: PathBuf,
}

impl Serving {
    /// Start `stado <args>` in the isolated host and keep it running.
    pub fn start(host: &Host, args: &[&str]) -> Self {
        let output = host
            .root
            .path()
            .join(format!("serving-{}.log", args.join("-")));
        let log = std::fs::File::create(&output).expect("an output file for a served command");
        let errors = log.try_clone().expect("one file for both streams");
        let child = host
            .command(args)
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(errors))
            .spawn()
            .expect("the built stado binary spawns");
        Self { child, output }
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    pub fn running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Everything the process has written so far, for a failure message.
    pub fn said(&self) -> String {
        std::fs::read_to_string(&self.output).unwrap_or_default()
    }

    /// End it the way a crash ends it: no chance to publish a last word, so
    /// the state file keeps saying whatever it said while the process lived.
    pub fn end(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Serving {
    fn drop(&mut self) {
        self.end();
    }
}

fn loopback(port: u16) -> std::net::SocketAddr {
    format!("127.0.0.1:{port}")
        .to_socket_addrs()
        .expect("a loopback address")
        .next()
        .expect("a loopback address")
}

/// Whether something accepts on a loopback port right now. A port nothing
/// holds refuses immediately; there is no network in between.
pub fn listening(port: u16) -> bool {
    TcpStream::connect(loopback(port)).is_ok()
}

/// Wait, bounded, until something accepts on a loopback port.
pub fn wait_listening(port: u16) -> bool {
    wait_until(|| listening(port))
}

/// Wait, bounded, until the resolver has published the state named, and
/// return what it published.
pub fn wait_published(host: &Host, state: &str) -> Value {
    let mut last = Value::Null;
    let found = wait_until(|| match host.published_state() {
        Some(published) => {
            let matched = published["state"] == state;
            last = published;
            matched
        }
        None => false,
    });
    assert!(
        found,
        "no resolver published state {state:?}; the file says {last}"
    );
    last
}

fn wait_until(mut ready: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + BUDGET;
    while Instant::now() < deadline {
        if ready() {
            return true;
        }
        std::thread::sleep(POLL);
    }
    false
}

/// One HTTP/1.1 GET on a fresh connection, and whatever comes back. The test
/// holds the client end, so the answer is the served program's own.
pub fn http_get(port: u16, path: &str, headers: &[(&str, &str)]) -> Result<String, String> {
    let mut stream =
        TcpStream::connect(loopback(port)).map_err(|error| format!("connect: {error}"))?;
    let mut request = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|error| format!("write: {error}"))?;
    let mut answer = Vec::new();
    stream
        .read_to_end(&mut answer)
        .map_err(|error| format!("read: {error}"))?;
    Ok(String::from_utf8_lossy(&answer).into_owned())
}

/// Direct children of a process, by program name — the count that used to
/// grow with traffic. Read with the operating system's own tools.
pub fn children_named(pid: u32, program: &str) -> usize {
    let listed = Command::new("pgrep")
        .args(["-P", &pid.to_string()])
        .output()
        .expect("the real pgrep runs");
    String::from_utf8_lossy(&listed.stdout)
        .split_whitespace()
        .filter(|child| {
            let named = Command::new("ps")
                .args(["-p", child, "-o", "comm="])
                .output()
                .expect("the real ps runs");
            String::from_utf8_lossy(&named.stdout).contains(program)
        })
        .count()
}
