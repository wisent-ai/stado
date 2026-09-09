//! The real listeners this area binds, and the real probe it reads this
//! machine's tailscale CLI through.
//!
//! An upstream nothing is listening on is a declaration about a fiction, so a
//! case that declares one binds the socket itself. And the edge reading is
//! taken from the product's OWN HTTP surface running on this machine's
//! loopback, never from a hand-written answer standing in for a service: what
//! a case asserts is the sentence the product wrote about the answer it really
//! received.

use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::fixture::{Fixture, SYSTEM_PATH};

/// The tailscale CLI as the product's own allowlist spells the program, so
/// this area and `stado host exec` cannot disagree about which binary a host's
/// tailscale CLI is or which paths it may live at. A declared name copied from
/// that table, not constants, config or tuning.
pub const TAILSCALE_PROGRAM: &str = "/usr/bin/tailscale";

/// How long a case waits for a listener it started to accept. Generous: the
/// machine may be compiling another area at the same time.
const LISTENER_WAIT: Duration = Duration::from_secs(60);

/// A loopback port nothing is listening on, taken by binding and releasing it.
pub fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind a loopback port")
        .local_addr()
        .expect("the bound socket has an address")
        .port()
}

pub fn accepts(port: u16) -> bool {
    TcpStream::connect(SocketAddr::from(([127, 0, 0, 1], port))).is_ok()
}

pub fn await_listener(port: u16, what: &str) {
    let deadline = Instant::now() + LISTENER_WAIT;
    while Instant::now() < deadline {
        if accepts(port) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("blocked: {what} never accepted a connection on 127.0.0.1:{port}");
}

/// A loopback listener this test really binds, so a declared upstream names a
/// connection that is genuinely accepting while the case runs. The socket goes
/// away with this value, so nothing is left listening afterwards.
pub struct Upstream {
    listener: TcpListener,
}

impl Upstream {
    pub fn bind() -> Self {
        Self {
            listener: TcpListener::bind("127.0.0.1:0").expect("bind a loopback upstream"),
        }
    }

    pub fn port(&self) -> u16 {
        self.listener
            .local_addr()
            .expect("the upstream socket has an address")
            .port()
    }

    pub fn origin(&self) -> String {
        format!("http://127.0.0.1:{}", self.port())
    }
}

/// The product's own HTTP service, running on a loopback port of this machine
/// over the same canonical store the commands read.
pub struct Service {
    child: Child,
    port: u16,
}

impl Service {
    pub fn start(fixture: &Fixture) -> Self {
        let port = free_port();
        let child = Command::new(env!("CARGO_BIN_EXE_stado"))
            .args([
                "dashboard",
                "--bind",
                "127.0.0.1",
                "--port",
                &port.to_string(),
            ])
            .env_clear()
            .env("HOME", fixture.home())
            .env("PATH", SYSTEM_PATH)
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", fixture.store())
            .env("STADO_CONFIG", fixture.home().join("no-such-config.json"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("the built stado dashboard starts");
        let service = Self { child, port };
        await_listener(port, "the product's own dashboard");
        service
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    pub fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Whether this machine carries the tailscale CLI at one of the paths the
/// product's own allowlist declares for it. A machine that carries it at none
/// of them is a real state too, and the publication read has its own sentence
/// for that.
pub fn tailscale_cli() -> Option<&'static str> {
    stado::deploy::host_exec::program_candidates(TAILSCALE_PROGRAM)
        .expect("tailscale is in the product's own program candidate table")
        .iter()
        .copied()
        .find(|candidate| Path::new(candidate).is_file())
}
