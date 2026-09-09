//! The listener under test — `stado dashboard --bind 127.0.0.1 --port N`, run
//! as its own process — and the requests and served documents the cases read.
//!
//! Isolation is environmental, as everywhere else in this suite:
//! `WC_STORAGE_BACKEND=local` plus `WC_LOCAL_STORAGE_PATH=<TempDir>`, a
//! set-but-missing `STADO_CONFIG`, `HOME` inside the temp dir, owner-only
//! grant files in it, and every Skarbiec endpoint the case is not about
//! pointed at a loopback port nothing listens on. Nothing here can reach the
//! operator's real vault, registry, store or fleet.

#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::policy;
use crate::vault::reserved_port;

/// Seconds a shut boundary waits before a request may revalidate it. Long
/// enough that a burst inside the window is decided by the recorded verdict
/// rather than by scheduling luck, short enough to keep a case a few seconds.
pub const COOLDOWN_SECONDS: u64 = 3;

/// How long a case waits for the boot sweep to record its verdicts. The
/// listener accepts before the sweep finishes, by design, so the wait is on
/// the served document rather than on the socket.
const BOOT_WAIT: Duration = Duration::from_secs(120);

/// Wait out one revalidation cooldown.
///
/// The cooldown is anchored on the last attempt, and the boot sweep is an
/// attempt, so a case that wants a request to actually revalidate must let
/// the window pass first. The margin is scheduling slack, not tuning.
pub fn past_cooldown() {
    std::thread::sleep(Duration::from_secs(COOLDOWN_SECONDS) + Duration::from_millis(400));
}

/// The body every object route answers while the object boundary is shut.
/// Copied from the wire: the fleet's clients and the incident vocabulary both
/// match on this exact string.
pub const OBJECT_UNAVAILABLE: &str = r#"{"error":"object authorization unavailable"}"#;

pub struct Answer {
    pub status: u16,
    pub body: String,
}

impl Answer {
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.body)
            .unwrap_or_else(|error| panic!("answer is not JSON ({error}): {}", self.body))
    }
}

/// The isolated environment one case runs the listener in.
pub struct Env {
    root: tempfile::TempDir,
}

impl Env {
    pub fn new() -> Self {
        let root = tempfile::tempdir().expect("an isolated root");
        let env = Self { root };
        std::fs::create_dir_all(env.home()).expect("create the isolated home");
        std::fs::create_dir_all(env.store()).expect("create the isolated store");
        // The coordinator grant is never used by a verifier; it exists because
        // every verifier refuses a grant file it shares with the coordinator.
        std::fs::write(env.grant("coordinator"), "coordinator-grant-unused")
            .expect("write the coordinator grant");
        env
    }

    pub fn home(&self) -> PathBuf {
        self.root.path().join("home")
    }

    pub fn store(&self) -> PathBuf {
        self.root.path().join("store")
    }

    /// One named grant file inside the isolated home.
    pub fn grant(&self, name: &str) -> PathBuf {
        self.home().join(format!("{name}-grant"))
    }

    /// Start the listener. `object_vault` and `release_vault` are the endpoints
    /// those two verifiers read; a case whose subject is an unreachable vault
    /// passes a port nothing listens on.
    pub fn start(&self, object_vault: &str, release_vault: &str) -> Listener {
        let port = reserved_port();
        let dead = format!("http://127.0.0.1:{}", reserved_port());
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .args([
                "dashboard",
                "--bind",
                "127.0.0.1",
                "--port",
                &port.to_string(),
            ])
            .env_clear()
            .env(
                "PATH",
                std::env::var("PATH").expect("the caller has a PATH"),
            )
            .env("HOME", self.home())
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.store())
            .env("WC_STADO_STORAGE_NAMESPACE", "boundary-area")
            .env("STADO_CONFIG", self.home().join("no-such-config.json"))
            .env("WC_OBJECT_API_NAMESPACES", policy::namespaces())
            .env("WC_RELEASE_API_PUBLISHERS", policy::publishers())
            .env("WC_SKARBIEC_URL", &dead)
            .env("WC_SKARBIEC_TOKEN_FILE", self.grant("coordinator"))
            .env("WC_OBJECT_SKARBIEC_URL", object_vault)
            .env(
                "WC_OBJECT_SKARBIEC_CONSUMER",
                stado::config::OBJECT_API_VERIFIER_CONSUMER,
            )
            .env("WC_OBJECT_SKARBIEC_TOKEN_FILE", self.grant("object"))
            .env("WC_RELEASE_SKARBIEC_URL", release_vault)
            .env(
                "WC_RELEASE_SKARBIEC_CONSUMER",
                stado::config::RELEASE_API_VERIFIER_CONSUMER,
            )
            .env("WC_RELEASE_SKARBIEC_TOKEN_FILE", self.grant("release"))
            .env("WC_MACHINE_SKARBIEC_URL", &dead)
            .env("WC_SERVICE_SKARBIEC_URL", &dead)
            .env("WC_RATE_LIMIT_SKARBIEC_URL", &dead)
            .env("WC_REGISTRY_SKARBIEC_URL", &dead)
            .env("WC_INTEGRATION_SKARBIEC_URL", &dead)
            .env("WC_INTEGRATION_PROVIDER_SKARBIEC_URL", &dead)
            .env("WC_DASHBOARD_BOUNDARY_ATTEMPTS", "1")
            .env(
                "WC_DASHBOARD_BOUNDARY_RECHECK_SECONDS",
                COOLDOWN_SECONDS.to_string(),
            )
            .env("WC_VAST_AUTO_LIST", "false")
            .stdin(Stdio::null())
            .stdout(log(&self.home().join("dashboard.out")))
            .stderr(log(&self.home().join("dashboard.err")));
        let child = command.spawn().expect("the built stado binary starts");
        let listener = Listener {
            addr: SocketAddr::from(([127, 0, 0, 1], port)),
            child,
            log: self.home().join("dashboard.err"),
        };
        listener.await_boot_verdicts();
        listener
    }
}

fn log(path: &Path) -> Stdio {
    Stdio::from(std::fs::File::create(path).expect("create the listener log"))
}

pub struct Listener {
    addr: SocketAddr,
    child: Child,
    log: PathBuf,
}

impl Listener {
    /// The boot sweep runs beside the accept loop, so a case waits for the
    /// verdicts it is about to read to exist at all.
    fn await_boot_verdicts(&self) {
        let deadline = Instant::now() + BOOT_WAIT;
        loop {
            let settled = self.state()["boundaries"]
                .as_object()
                .expect("the state document lists boundaries")
                .values()
                .all(|boundary| boundary["checked_at"].is_string());
            if settled {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "the listener recorded no boot verdict within {} seconds:\n{}",
                BOOT_WAIT.as_secs(),
                self.logged()
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    pub fn logged(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    /// The operator's own read of every boundary's verdict.
    pub fn state(&self) -> Value {
        let answer = self.request("GET", "/api/state.json", None, None);
        assert_eq!(answer.status, 200, "state document: {}", answer.body);
        answer.json()
    }

    /// One boundary as `/api/state.json` publishes it.
    pub fn boundary(&self, key: &str) -> Value {
        self.state()["boundaries"][key].clone()
    }

    /// The process id the listener publishes about itself, so a case can prove
    /// that recovery happened inside this very process.
    pub fn pid(&self) -> u64 {
        self.state()["storage"]["pid"]
            .as_u64()
            .expect("the state document names the serving process")
    }

    pub fn get(&self, target: &str, bearer: Option<&str>) -> Answer {
        self.request("GET", target, bearer, None)
    }

    pub fn post(&self, target: &str, body: &str) -> Answer {
        self.request("POST", target, None, Some(body))
    }

    /// One request against the listener, with the loopback `Host` its guard
    /// requires.
    fn request(
        &self,
        method: &str,
        target: &str,
        bearer: Option<&str>,
        body: Option<&str>,
    ) -> Answer {
        let deadline = Instant::now() + BOOT_WAIT;
        let mut stream = loop {
            match TcpStream::connect(self.addr) {
                Ok(stream) => break stream,
                Err(error) => {
                    assert!(
                        Instant::now() < deadline,
                        "the listener never accepted a loopback connection: {error}\n{}",
                        self.logged()
                    );
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        };
        let mut request = format!(
            "{method} {target} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n",
            self.addr
        );
        if let Some(bearer) = bearer {
            request.push_str(&format!("Authorization: Bearer {bearer}\r\n"));
        }
        let body = body.unwrap_or("");
        request.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ));
        stream
            .write_all(request.as_bytes())
            .expect("the listener accepts the request");
        let mut raw = Vec::new();
        stream
            .read_to_end(&mut raw)
            .expect("the listener answers and closes");
        let raw = String::from_utf8_lossy(&raw).into_owned();
        let (head, body) = raw
            .split_once("\r\n\r\n")
            .unwrap_or_else(|| panic!("the answer has no head and body: {raw:?}"));
        Answer {
            status: head
                .split_whitespace()
                .nth(1)
                .and_then(|status| status.parse().ok())
                .expect("the status line carries a code"),
            body: body.to_string(),
        }
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
