//! One real Stado listener with its own object store, `HOME` and session
//! ledger, the real installed Jeden runtime beside it, and one real WebSocket
//! attachment. Split from the cases because this repository caps a source
//! file at three hundred lines.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use futures::SinkExt;
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::{handshake::client::generate_key, http, Message};

pub const TARGET: &str = "workload-stream-host";
pub const KIND: &str = "jeden-session";
/// Bound on the listener's readiness, one stream answer, and the teardown.
pub const DEADLINE: Duration = Duration::from_secs(30);
pub const POLL: Duration = Duration::from_millis(100);
/// The only registry document version the product accepts.
const REGISTRY_SCHEMA_VERSION: u64 = 2;
const ACTION_HEADER: &str = "x-stado-action";
const ACTION: &str = "workload-attach";

/// The server needs only the handshake, so this client opens its own socket
/// rather than making the product depend on the connector.
pub type Socket = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;
pub struct Area {
    pub root: PathBuf,
    pub home: PathBuf,
    address: String,
    listener: std::process::Child,
}

impl Area {
    pub async fn start() -> Self {
        let evidence = Path::new(env!("CARGO_MANIFEST_DIR")).join(".wisent-output/workload-stream");
        std::fs::create_dir_all(&evidence).expect("create retained stream evidence directory");
        let root = evidence.join(uuid::Uuid::new_v4().to_string());
        let home = root.join("home");
        let storage = root.join("storage");
        let bin = home.join(".stado/bin");
        for directory in [&home, &storage, &bin, &root.join("temporary")] {
            std::fs::create_dir_all(directory).expect("create isolated stream directory");
        }
        let jeden = installed_jeden();
        std::os::unix::fs::symlink(&jeden, bin.join("jeden")).expect("install the real Jeden");
        std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_stado"), bin.join("stado"))
            .expect("install the tested Stado");
        std::fs::write(
            storage.join("registry.json"),
            serde_json::to_vec_pretty(&json!({
                "schema_version": REGISTRY_SCHEMA_VERSION,
                "targets": [{
                    "name": TARGET,
                    "kind": "local",
                    "ssh": Value::Null,
                    "release_platform": release_platform(),
                    "hostnames": [hostname()],
                }],
                "coordinators": [],
            }))
            .unwrap(),
        )
        .expect("write the isolated registry");
        let config = root.join("config.json");
        std::fs::write(
            &config,
            serde_json::to_vec(&json!({
                "storage": {"backend": "local", "local": {"path": storage}},
            }))
            .unwrap(),
        )
        .expect("write the isolated deployment profile");
        std::fs::write(
            root.join("source.json"),
            serde_json::to_vec_pretty(&json!({
                "stado": env!("CARGO_BIN_EXE_stado"),
                "jeden": jeden,
            }))
            .unwrap(),
        )
        .expect("record the binaries under test");
        let log = root.join("listener.stderr");
        let listener = Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(["dashboard", "--bind", "127.0.0.1", "--port", "0"])
            .env_clear()
            .env("HOME", &home)
            .env("TMPDIR", root.join("temporary"))
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", bin.display()),
            )
            .env("STADO_CONFIG", &config)
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &storage)
            .env("WC_PROVIDERS", "local")
            .env("NO_COLOR", "1")
            .env("JEDEN_SESSION_ROOT", home.join(".jeden/sessions"))
            .stdin(Stdio::null())
            .stdout(Stdio::from(
                std::fs::File::create(root.join("listener.stdout")).unwrap(),
            ))
            .stderr(Stdio::from(std::fs::File::create(&log).unwrap()))
            .spawn()
            .expect("the real Stado listener starts");
        let mut area = Self {
            root,
            home,
            address: String::new(),
            listener,
        };
        area.address = area.wait_for_address(&log).await;
        eprintln!("workload stream evidence: {}", area.root.display());
        area
    }

    async fn wait_for_address(&mut self, log: &Path) -> String {
        let deadline = tokio::time::Instant::now() + DEADLINE;
        loop {
            assert!(
                self.listener.try_wait().unwrap().is_none(),
                "the listener exited; retained logs: {}",
                self.root.display()
            );
            if let Some(address) = std::fs::read_to_string(log)
                .unwrap_or_default()
                .lines()
                .find_map(|line| line.strip_prefix("[dashboard] listening on http://"))
            {
                return address.trim().to_string();
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the listener never reported an address; retained logs: {}",
                self.root.display()
            );
            tokio::time::sleep(POLL).await;
        }
    }

    /// One attachment request on a real WebSocket, carrying the route's own
    /// action header.
    pub async fn attach(&self, request: Value) -> Socket {
        let wire = http::Request::builder()
            .uri(format!(
                "ws://{}/api/operator/workload/attach",
                self.address
            ))
            .header("Host", &self.address)
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Version", "13")
            .header("Sec-WebSocket-Key", generate_key())
            .header(ACTION_HEADER, ACTION)
            .body(())
            .expect("a WebSocket upgrade request");
        let stream = tokio::net::TcpStream::connect(&self.address)
            .await
            .expect("the listener accepted a connection");
        let (mut socket, _) = tokio_tungstenite::client_async(wire, stream)
            .await
            .expect("the listener accepted the upgrade");
        socket
            .send(Message::text(request.to_string()))
            .await
            .expect("the attachment request was sent");
        socket
    }

    /// The attachment request this area sends, with the caller's confirmation.
    pub fn request(confirmation: &str) -> Value {
        json!({
            "kind": KIND,
            "target": TARGET,
            "workspace": "__home__",
            "confirmation": confirmation,
        })
    }

    /// Every process on this machine running this area's own Jeden, by pid.
    pub fn attached_jeden(&self) -> Vec<String> {
        let marker = self.home.join(".stado/bin/jeden").display().to_string();
        let listing = Command::new("/bin/ps")
            .args(["-A", "-o", "pid=,command="])
            .output()
            .expect("the process table is readable");
        String::from_utf8_lossy(&listing.stdout)
            .lines()
            .filter(|line| line.contains(&marker))
            .map(|line| {
                line.split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }

    /// The session ledgers Jeden wrote under this area's home.
    pub fn ledgers(&self) -> Vec<String> {
        std::fs::read_dir(self.home.join(".jeden/sessions"))
            .map(|entries| {
                entries
                    .filter_map(|entry| {
                        Some(entry.ok()?.file_name().to_string_lossy().into_owned())
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn retain(&self, name: &str, text: &str) {
        std::fs::write(self.root.join(name), text).expect("retain the stream evidence");
    }
}

impl Drop for Area {
    fn drop(&mut self) {
        let _ = self.listener.kill();
        let _ = self.listener.wait();
    }
}

fn hostname() -> String {
    let output = Command::new("hostname")
        .output()
        .expect("the real hostname executable runs");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn release_platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("linux", "x86_64") => "linux-amd64",
        (os, arch) => panic!(
            "blocked: the workload stream area runs the current host and requires macOS arm64 \
             or Linux amd64, got {os}-{arch}"
        ),
    }
}

/// The real installed Jeden runtime this fleet runs.
fn installed_jeden() -> PathBuf {
    let declared = std::env::var_os("JEDEN_BIN").map(PathBuf::from);
    let binary = declared.unwrap_or_else(|| {
        PathBuf::from(std::env::var_os("HOME").expect("HOME is set")).join(".stado/bin/jeden")
    });
    let binary = std::fs::canonicalize(&binary).unwrap_or_else(|error| {
        panic!(
            "blocked: the real installed Jeden runtime is required for this area: {} ({error})",
            binary.display()
        )
    });
    let identity = Command::new(&binary)
        .arg("--version")
        .output()
        .expect("the installed Jeden runtime starts");
    assert!(
        identity.status.success(),
        "the installed Jeden refused --version"
    );
    binary
}
