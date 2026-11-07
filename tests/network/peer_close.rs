//! `stado network connect` ends when its peer has said everything, though its
//! own standard input stays open: OpenSSH's `ProxyCommand` keeps that input
//! open for as long as `ssh` runs, so a connect that waited for it outlived
//! the remote side and held the `ssh` it served. A port the peer's registry
//! paths do not name is refused by the peer's node, with the ports it does.
//!
//! Two accounts of this machine, each with its own HOME, share one isolated
//! local store whose registry declares `example-host`, the account that
//! connects, and `example-peer`, whose node (`stado network serve`, run by the
//! journey from the same build) listens on a port the system hands out and is
//! declared at that loopback endpoint. Its SSH path names the port of a
//! listener this journey opens on the loopback, which answers one line and
//! closes. Both key pairs are real X25519 pairs. Nothing of the operator's is
//! read or written.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use base64::Engine;

/// The Noise pattern the Stado network speaks, for the accounts' key pairs.
const NOISE_PARAMS: &str = "Noise_IK_25519_ChaChaPoly_BLAKE2s";
/// What the peer's listener answers before it closes.
const ANSWER: &str = "the peer answers and closes\n";
/// How the peer's node refuses a port its registry paths do not name.
const UNDECLARED: &str = "is not an SSH port registry host example-peer declares";

/// A port the system hands out on the loopback, free once returned.
fn free_port() -> u16 {
    TcpListener::bind((Ipv4Addr::LOCALHOST, u16::MIN))
        .and_then(|free| free.local_addr())
        .expect("a port the system hands out")
        .port()
}

/// A listener on the loopback that answers each connection with [`ANSWER`]
/// and closes it.
fn answering_listener() -> u16 {
    let listener =
        TcpListener::bind((Ipv4Addr::LOCALHOST, u16::MIN)).expect("listen on the loopback");
    let port = listener
        .local_addr()
        .expect("the listener's address")
        .port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let _ = stream.write_all(ANSWER.as_bytes());
        }
    });
    port
}

struct Fixture {
    root: PathBuf,
    node: Option<Child>,
}

impl Fixture {
    /// Both accounts' keys and the shared registry; the peer's node listens
    /// on `node_port` and its SSH path names `ssh_port`.
    fn new(name: &str, node_port: u16, ssh_port: u16) -> Self {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("network-peer-close-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("store")).expect("create the store");
        let host = Self::account(&root, "host");
        let peer = Self::account(&root, "peer");
        std::fs::write(
            root.join("peer/.stado/network/node.json"),
            serde_json::json!({"host": "example-peer", "port": node_port}).to_string(),
        )
        .expect("record the peer's node");
        let registry = serde_json::json!({
            "schema_version": 2,
            "targets": [
                {
                    "name": "example-host",
                    "kind": "local",
                    "ssh": "operator@example-host.invalid",
                    "release_platform": "darwin-arm64",
                    "hostnames": ["example-host.invalid"],
                    "network": {"public_key": host}
                },
                {
                    "name": "example-peer",
                    "kind": "local",
                    "ssh": format!("operator@[example-peer.stado.internal]:{ssh_port}"),
                    "release_platform": "darwin-arm64",
                    "hostnames": ["example-peer.invalid"],
                    "network": {
                        "public_key": peer,
                        "endpoints": [format!("{}:{node_port}", Ipv4Addr::LOCALHOST)]
                    }
                }
            ],
            "coordinators": [],
            "service_directory": serde_json::Value::Null
        });
        std::fs::write(root.join("store/registry.json"), registry.to_string())
            .expect("seed the registry");
        Self { root, node: None }
    }

    /// An account's HOME with a fresh key pair; its public half.
    fn account(root: &Path, name: &str) -> String {
        let network = root.join(name).join(".stado/network");
        std::fs::create_dir_all(&network).expect("create the network directory");
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
        .expect("write the account's network key");
        std::fs::write(root.join(name).join("config.json"), "{}").expect("write the config");
        public
    }

    fn stado(&self, account: &str, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .args(args)
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.root.join("store"))
            .env("STADO_CONFIG", self.root.join(account).join("config.json"))
            .env("HOME", self.root.join(account));
        command
    }

    /// The peer's node, once it says it listens.
    fn serve_peer(&mut self) {
        let mut node = self
            .stado("peer", &["network", "serve"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the node starts");
        let mut said = BufReader::new(node.stderr.take().expect("the node's stderr"));
        let mut line = String::new();
        while !line.contains("listening on port") {
            line.clear();
            let read = said.read_line(&mut line).expect("read the node's stderr");
            assert!(read > usize::MIN, "the node ended before it listened");
        }
        // The node keeps writing per-connection lines; they are drained so it
        // never blocks on a full pipe.
        std::thread::spawn(move || for _ in said.lines() {});
        self.node = Some(node);
    }

    /// `stado network connect example-peer PORT` from the connecting account,
    /// its standard input held open until it has ended: its status, stdout
    /// and stderr.
    fn connect(&self, port: u16) -> (std::process::ExitStatus, String, String) {
        let port = port.to_string();
        let mut connect = self
            .stado(
                "host",
                &["network", "connect", "example-peer.stado.internal", &port],
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the stado binary runs");
        let held = connect.stdin.take().expect("connect's standard input");
        let mut stdout = String::new();
        connect
            .stdout
            .take()
            .expect("connect's stdout")
            .read_to_string(&mut stdout)
            .expect("read connect's stdout");
        let output = connect.wait_with_output().expect("connect ends");
        drop(held);
        (
            output.status,
            stdout,
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
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
fn connect_ends_with_the_peers_answer_while_its_input_stays_open() {
    let ssh_port = answering_listener();
    let node_port = free_port();
    let mut fixture = Fixture::new("answer", node_port, ssh_port);
    fixture.serve_peer();
    let (status, stdout, stderr) = fixture.connect(ssh_port);
    assert!(
        status.success(),
        "connect ends with the peer's answer once the peer closed: {stderr}"
    );
    assert_eq!(
        stdout, ANSWER,
        "the peer's whole answer is carried: {stderr}"
    );

    let (status, stdout, stderr) = fixture.connect(node_port);
    assert!(
        !status.success(),
        "a port the peer's paths do not name is refused: {stdout} {stderr}"
    );
    assert!(
        stderr.contains(&format!("port {node_port} {UNDECLARED}"))
            && stderr.contains(&format!("its paths name [{ssh_port}]")),
        "the refusal names the port asked for and the ports the peer opens: {stderr}"
    );
}
