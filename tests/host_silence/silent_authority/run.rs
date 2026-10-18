//! The isolated run `main.rs` drives: the real stado with `HOME` under the
//! checkout's build directory, a last-known-good registry copy naming this
//! machine as the reader, and a registry authority that accepts every SSH
//! connection and sends nothing.
use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};

pub(crate) const AUTHORITY: &str = "other-host";
const AUTHORITY_STADO: &str = "/usr/local/bin/stado";
pub(crate) const ABANDONED: &str = "request abandoned unanswered";

pub(crate) struct Run {
    pub(crate) root: PathBuf,
    /// How many connections the authority accepted; the listener's thread
    /// keeps each one open, so the authority stays silent rather than closed.
    accepted: Arc<AtomicUsize>,
}

impl Run {
    /// A run of its own, named so two journeys of one test binary never
    /// share a home.
    pub(crate) fn new(name: &str) -> Self {
        let parent = Path::new(env!("CARGO_TARGET_TMPDIR")).join("host-silence-silent-authority");
        std::fs::create_dir_all(&parent).expect("create the test output directory");
        let root = parent.join(format!("{name}-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".stado/cache")).expect("create the run directory");
        std::fs::create_dir_all(root.join("tmp")).expect("create the run's scratch directory");
        Self {
            root,
            accepted: Arc::default(),
        }
    }

    /// How many connections the silent authority has accepted so far.
    pub(crate) fn accepted(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }

    /// The real stado with nothing of this account's configuration.
    pub(crate) fn stado(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .current_dir(&self.root)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", &self.root)
            .env("STADO_CONFIG", self.root.join("no-config.toml"))
            .env("TMPDIR", self.root.join("tmp"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    /// A loopback port this machine hands out now, as the product binds one.
    fn free_port(&self) -> u16 {
        let output = self
            .stado()
            .args(["host", "local", "port"])
            .output()
            .expect("run the real stado");
        let printed = String::from_utf8_lossy(&output.stdout).into_owned();
        match printed.trim().parse() {
            Ok(port) => port,
            Err(error) => panic!("stado host local port printed {printed:?}: {error}"),
        }
    }

    /// The authority's SSH destination: a listener that accepts and never
    /// speaks, so the SSH client waits for a banner that never comes. A
    /// connection the listener cannot accept ends the test naming why.
    pub(crate) fn silent_authority(&self) -> String {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, self.free_port()))
            .expect("bind the loopback port this machine handed out");
        let address = listener.local_addr().unwrap().to_string();
        let accepted = Arc::clone(&self.accepted);
        std::thread::spawn(move || {
            let mut open = Vec::new();
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => open.push(stream),
                    Err(error) => panic!("the silent authority could not accept: {error}"),
                }
                accepted.store(open.len(), Ordering::SeqCst);
            }
        });
        address
    }

    /// The copy this stado resolves its authority through: the shared
    /// directory template with this machine as the reader and the silent
    /// listener as the authority's SSH destination. Answers the refresh
    /// interval the reader declares, the one measure of the wait.
    pub(crate) fn write_last_good(&self, authority_ssh: &str) -> i64 {
        let mut document: Value =
            serde_json::from_str(include_str!("../../service_directory/registry.json")).unwrap();
        let targets = document["targets"].as_array_mut().unwrap();
        let reader = targets
            .first_mut()
            .expect("the template declares the reader first");
        let refresh_seconds = reader["service_resolver"]["refresh_seconds"]
            .as_i64()
            .expect("the template declares the reader's refresh interval");
        reader["hostnames"] = json!([hostname()]);
        reader["services"] = json!([]);
        let authority = targets
            .last_mut()
            .expect("the template declares the authority last");
        authority["name"] = json!(AUTHORITY);
        authority["ssh"] = json!(authority_ssh);
        authority["services"] = json!([]);
        document["service_directory"]["authority"] =
            json!({"target": AUTHORITY, "command": AUTHORITY_STADO});
        document["service_directory"]["services"] = json!({});
        std::fs::write(
            self.root.join(".stado/cache/registry-last-good.json"),
            serde_json::to_vec_pretty(&document).unwrap(),
        )
        .unwrap();
        std::fs::write(
            self.root.join(".stado/cache/registry-last-good.meta.json"),
            serde_json::to_vec(&json!({
                "read_at": "1970-01-01T00:00:00Z",
                "generation": "example",
            }))
            .unwrap(),
        )
        .unwrap();
        refresh_seconds
    }

    pub(crate) fn registry_self(&self) -> Output {
        let output = self
            .stado()
            .args(["registry", "self", "--name-only"])
            .output()
            .expect("run the real stado");
        std::fs::write(self.root.join("registry-self.stderr"), &output.stderr)
            .expect("keep the command's stderr beside the run");
        output
    }
}

pub(crate) fn hostname() -> String {
    let output = Command::new("hostname").output().unwrap();
    String::from_utf8(output.stdout)
        .unwrap()
        .trim()
        .to_lowercase()
}
