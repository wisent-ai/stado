//! A Stado connection to a peer on this machine's local network reaches the
//! peer or, refused, names what macOS decided for the binary, and is also
//! asked of this host's own network node, which opens it for the process.
//!
//! The built stado runs `network connect` against an isolated local store
//! whose registry declares this host, `example-host`, and one peer,
//! `example-peer`, with its one network endpoint at a listener this journey
//! opens on this machine's own LAN address: an IPv4 address of an interface
//! that is up, not loopback, and on a private or link-local network. The
//! listener answers with a line that is no Noise handshake, so a connection
//! that reached it ends in a handshake failure naming the endpoint, after the
//! listener saw it. The account key is a real X25519 pair, so this host's node
//! (`stado network serve`, run by the journey from the same build) admits the
//! process. HOME, the store and STADO_CONFIG sit under the test's own
//! directory, so no operator state is read or written. A machine with no LAN
//! address is a failed run.

use std::io::{BufRead, BufReader, Write};
use std::net::{IpAddr, Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;

use base64::Engine;
use nix::net::if_::InterfaceFlags;

/// The sentence a refusal is named with when macOS has not allowed the binary.
const DENIAL: &str =
    "needs Local Network access in System Settings > Privacy & Security > Local Network";

/// The sentence a refusal is named with when macOS allowed the binary, so the
/// peer itself did not answer.
const PEER_SILENT: &str = "itself did not answer";

/// The Noise pattern the Stado network speaks, for the account's key pair.
const NOISE_PARAMS: &str = "Noise_IK_25519_ChaChaPoly_BLAKE2s";

/// An IPv4 address this machine holds on a local network.
fn lan_address() -> Ipv4Addr {
    nix::ifaddrs::getifaddrs()
        .expect("read this machine's interfaces")
        .filter(|interface| {
            interface.flags.contains(InterfaceFlags::IFF_UP)
                && !interface.flags.contains(InterfaceFlags::IFF_LOOPBACK)
        })
        .filter_map(|interface| {
            interface
                .address?
                .as_sockaddr_in()
                .map(|address| address.ip())
        })
        .find(|address| address.is_private() || address.is_link_local())
        .expect(
            "this machine holds an IPv4 address on a private or link-local network on an interface \
             that is up; without one there is no LAN peer to reach",
        )
}

/// A listener on the LAN address that reports each connection it accepts
/// and answers it with no Noise handshake.
fn peer_listener() -> (String, mpsc::Receiver<std::net::SocketAddr>) {
    let listener =
        TcpListener::bind((IpAddr::V4(lan_address()), 0)).expect("listen on the LAN address");
    let endpoint = listener
        .local_addr()
        .expect("the listener's address")
        .to_string();
    let (accepted, seen) = mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            // Said before answering, so it is in the channel before the
            // caller can read the answer and end.
            let from = stream.peer_addr().expect("the connection's address");
            if accepted.send(from).is_err() {
                return;
            }
            let _ = stream.write_all(b"this listener speaks no Noise handshake\n");
        }
    });
    (endpoint, seen)
}

struct Fixture {
    root: PathBuf,
    node: Option<Child>,
}

impl Fixture {
    fn new(name: &str, endpoint: &str) -> Self {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "network-local-network-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let network = root.join("home/.stado/network");
        std::fs::create_dir_all(&network).expect("create the network directory");
        std::fs::create_dir_all(root.join("store")).expect("create the store");
        let pair = snow::Builder::new(NOISE_PARAMS.parse().expect("the Noise pattern"))
            .generate_keypair()
            .expect("generate the account's key pair");
        let engine = base64::engine::general_purpose::STANDARD;
        let public = engine.encode(&pair.public);
        std::fs::write(
            network.join("key"),
            serde_json::json!({"private": engine.encode(&pair.private), "public": public})
                .to_string(),
        )
        .expect("write this account's network key");
        let registry = serde_json::json!({
            "schema_version": 2,
            "targets": [
                {
                    "name": "example-host",
                    "kind": "local",
                    "ssh": "operator@example-host.invalid",
                    "release_platform": "darwin-arm64",
                    "hostnames": ["example-host.invalid"],
                    "network": {"public_key": public}
                },
                {
                    "name": "example-peer",
                    "kind": "local",
                    "ssh": "operator@example-peer.stado.internal",
                    "release_platform": "darwin-arm64",
                    "hostnames": ["example-peer.invalid"],
                    "network": {"public_key": engine.encode([9u8; 32]), "endpoints": [endpoint]}
                }
            ],
            "coordinators": []
        });
        std::fs::write(root.join("store/registry.json"), registry.to_string())
            .expect("seed the registry");
        std::fs::write(root.join("config.json"), "{}").expect("write the empty config");
        Self { root, node: None }
    }

    fn stado(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .args(args)
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.root.join("store"))
            .env("STADO_CONFIG", self.root.join("config.json"))
            .env("HOME", self.root.join("home"))
            .stdin(Stdio::null());
        command
    }

    /// This host's node, serving on a port the system hands out, once it
    /// says it listens.
    fn serve_node(&mut self) {
        let port = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .and_then(|free| free.local_addr())
            .expect("a port the system hands out")
            .port();
        std::fs::write(
            self.root.join("home/.stado/network/node.json"),
            serde_json::json!({"host": "example-host", "port": port}).to_string(),
        )
        .expect("record this host's node");
        let mut node = self
            .stado(&["network", "serve"])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the node starts");
        let mut said = BufReader::new(node.stderr.take().expect("the node's stderr"));
        let mut line = String::new();
        while !line.contains("listening on port") {
            line.clear();
            let read = said.read_line(&mut line).expect("read the node's stderr");
            assert!(read > 0, "the node ended before it listened");
        }
        // The node keeps writing per-connection lines; they are drained so it
        // never blocks on a full pipe.
        std::thread::spawn(move || for _ in said.lines() {});
        self.node = Some(node);
    }

    fn connect(&self) -> std::process::Output {
        self.stado(&["network", "connect", "example-peer.stado.internal", "22"])
            .output()
            .expect("the stado binary runs")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(node) = self.node.as_mut() {
            let _ = node.kill();
            let _ = node.wait();
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn a_listening_lan_peer_is_reached_or_the_denial_is_named() {
    let (endpoint, seen) = peer_listener();
    let fixture = Fixture::new("direct", &endpoint);
    let output = fixture.connect();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "a peer that speaks no Noise is no connection: {stderr}"
    );
    assert!(
        stderr.contains(&format!("endpoint {endpoint}")),
        "the failure names the endpoint it tried: {stderr}"
    );
    assert!(
        !stderr.contains("this host's node"),
        "with no node recorded, no way through one is tried: {stderr}"
    );
    match seen.try_recv() {
        Ok(_) => assert!(
            !stderr.contains("No route to host"),
            "the listener was reached, so no route is missing: {stderr}"
        ),
        // The listener is up, so a refusal is macOS's: named as the binary not
        // allowed, or as a decision that could not be read, never as a peer
        // that did not answer.
        Err(_) => assert!(
            stderr.contains(&format!("{endpoint} is on this machine's local network"))
                && (stderr.contains(DENIAL) || stderr.contains("cannot be read"))
                && !stderr.contains(PEER_SILENT),
            "the listener saw no connection, so the failure names macOS's decision for the binary: {stderr}"
        ),
    }
}

#[test]
fn this_hosts_node_opens_the_connection_for_the_process() {
    let (endpoint, seen) = peer_listener();
    let mut fixture = Fixture::new("node", &endpoint);
    fixture.serve_node();
    let output = fixture.connect();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "a peer that speaks no Noise is no connection: {stderr}"
    );
    assert!(
        stderr.contains("this host's node example-host: the node refused: no way to example-peer"),
        "the node was asked, dialled the peer itself and named its failure: {stderr}"
    );
    assert!(
        stderr.matches(&format!("endpoint {endpoint}")).count() >= 2,
        "both the process and the node tried the peer's endpoint: {stderr}"
    );
    let reached: Vec<_> = seen.try_iter().collect();
    if !stderr.contains(DENIAL) {
        assert_eq!(
            reached.len(),
            2,
            "the process and the node each reached the listener: {stderr}"
        );
    }
}
