//! A registry read whose authority accepts the SSH connection and answers
//! nothing ends on its own, after the refresh interval this host's resolver
//! declares, refused with the interval and where to ask, and the reader
//! goes on to its last-known-good copy instead of standing for hours.
//!
//! The real `stado registry self` runs with an isolated `HOME` under the
//! checkout's build directory and a last-known-good registry copy that names
//! this machine as the reader and, as the registry authority, a loopback
//! listener this test owns that accepts every connection and sends nothing:
//! no SSH banner ever comes, so the authority's `stado resolver snapshot`
//! can never be asked and nothing of this host's resolver records the wait.
//! The test holds no clock: the command must end by itself.
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{json, Value};

const AUTHORITY: &str = "other-host";
const AUTHORITY_STADO: &str = "/usr/local/bin/stado";
const ABANDONED: &str = "request abandoned unanswered";

struct Run {
    root: PathBuf,
    /// The accepted connections, kept open so the authority stays silent
    /// rather than closed.
    held: std::sync::Arc<std::sync::Mutex<Vec<TcpStream>>>,
}

impl Run {
    fn new() -> Self {
        let parent = Path::new(env!("CARGO_TARGET_TMPDIR")).join("host-silence-silent-authority");
        std::fs::create_dir_all(&parent).expect("create the test output directory");
        let root = parent.join(std::process::id().to_string());
        std::fs::create_dir_all(root.join(".stado/cache")).expect("create the run directory");
        std::fs::create_dir_all(root.join("tmp")).expect("create the run's scratch directory");
        Self {
            root,
            held: Default::default(),
        }
    }

    /// The real stado with nothing of this account's configuration.
    fn stado(&self) -> Command {
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
    /// speaks, so the SSH client waits for a banner that never comes.
    fn silent_authority(&self) -> String {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, self.free_port()))
            .expect("bind the loopback port this machine handed out");
        let address = listener.local_addr().unwrap().to_string();
        let held = std::sync::Arc::clone(&self.held);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                held.lock().unwrap().push(stream);
            }
        });
        address
    }

    /// The copy this stado resolves its authority through: the shared
    /// directory template with this machine as the reader and the silent
    /// listener as the authority's SSH destination. Answers the refresh
    /// interval the reader declares, the one measure of the wait.
    fn write_last_good(&self, authority_ssh: &str) -> i64 {
        let mut document: Value =
            serde_json::from_str(include_str!("../service_directory/registry.json")).unwrap();
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

    fn registry_self(&self) -> Output {
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

fn hostname() -> String {
    let output = Command::new("hostname").output().unwrap();
    String::from_utf8(output.stdout)
        .unwrap()
        .trim()
        .to_lowercase()
}

#[test]
fn a_silent_authority_is_abandoned_after_the_declared_refresh_interval() {
    let run = Run::new();
    let authority_ssh = run.silent_authority();
    let refresh_seconds = run.write_last_good(&authority_ssh);
    let output = run.registry_self();
    let said = String::from_utf8_lossy(&output.stderr).into_owned();

    assert!(
        said.contains(ABANDONED)
            && said.contains(AUTHORITY)
            && said.contains(&format!(
                "it stood longer than the {refresh_seconds}s refresh interval this host's resolver declares"
            ))
            && said.contains("`stado resolver snapshot` is what answered nothing")
            && said.contains(&format!("stado service logs com.wisent.stado --host {AUTHORITY}")),
        "the read is abandoned naming the authority, the interval it outlasted and where to ask: {said}"
    );
    assert!(
        said.contains("reading the last-known-good registry copy"),
        "the reader goes on to its last-known-good copy: {said}"
    );
    assert!(
        !run.held.lock().unwrap().is_empty(),
        "the authority accepted the SSH connection the read was abandoned on"
    );
    std::fs::write(
        run.root.join("report.json"),
        serde_json::to_vec_pretty(&json!({
            "authority_ssh": authority_ssh,
            "refresh_seconds": refresh_seconds,
            "exit_status": output.status.to_string(),
            "stderr": said,
            "stdout": String::from_utf8_lossy(&output.stdout),
        }))
        .unwrap(),
    )
    .unwrap();
}
