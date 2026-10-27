//! A registry read that cannot reach its authority answers with its own
//! refusal, and a fleet store that will not take the refusal's record never
//! replaces that answer. The record is written detached: a one-shot command
//! that exits first loses its copy, and the resolver on the same machine
//! records the same refusal from its long-lived process.
//!
//! The real `stado registry self` runs with an isolated `HOME` under the
//! checkout's build directory and a last-known-good registry copy that names
//! this machine as the reader and a host nobody can reach (`.invalid`) as the
//! registry authority, so the authority read fails over SSH and raises an
//! `authority_unreachable` refusal about that host. The fleet store is a
//! loopback listener this test owns that resets every connection after its
//! request line, so neither the registry download nor the refusal's record
//! lands.
use std::io::{BufRead, BufReader};
use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{json, Value};

/// The reader is the first target of the shared directory template; the
/// unreachable authority is its last.
const READER: &str = "marker-host";
const AUTHORITY: &str = "other-host";
const AUTHORITY_SSH: &str = "other-host.invalid";
const AUTHORITY_STADO: &str = "/usr/local/bin/stado";
const SSH_FAILED: &str = "registry authority SSH failed";

struct Run {
    root: PathBuf,
}

impl Run {
    fn new() -> Self {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../.build/journeys/host-silence-lost-refusal");
        std::fs::create_dir_all(&parent).expect("create the test output directory");
        let root = parent.join(std::process::id().to_string());
        std::fs::create_dir_all(root.join(".stado/cache")).expect("create the run directory");
        std::fs::create_dir_all(root.join("tmp")).expect("create the run's scratch directory");
        Self { root }
    }

    /// The real stado with nothing of this account's configuration.
    fn stado(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .current_dir(&self.root)
            .env_clear()
            .env(
                "PATH",
                std::env::var_os("PATH").expect("PATH names the programs stado runs"),
            )
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
        let output = stado_wait::output(self.stado().args(["host", "local", "port"]))
            .expect("run the real stado");
        let printed = String::from_utf8_lossy(&output.stdout).into_owned();
        match printed.trim().parse() {
            Ok(port) => port,
            Err(error) => panic!("stado host local port printed {printed:?}: {error}"),
        }
    }

    /// The fleet store the registry read and the refusal record go to: a
    /// loopback listener that resets every connection after its request
    /// line, so nothing is read from it and no write lands.
    fn resetting_store(&self) -> String {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, self.free_port()))
            .expect("bind the loopback port this machine handed out");
        let address = listener
            .local_addr()
            .expect("the bound listener has an address")
            .to_string();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let stream = match stream {
                    Ok(stream) => stream,
                    Err(error) => panic!("the resetting store stopped accepting: {error}"),
                };
                let mut request_line = String::new();
                if let Err(error) = BufReader::new(&stream).read_line(&mut request_line) {
                    eprintln!("the resetting store read no request line: {error}");
                }
                drop(stream);
            }
        });
        address
    }

    /// The copy this stado resolves its authority through: the shared
    /// directory template with this machine as the reader and the template's
    /// unreachable last target as the authority.
    fn write_last_good(&self) {
        let mut document: Value =
            serde_json::from_str(include_str!("../service_directory/registry.json"))
                .expect("the shared directory template is JSON");
        let targets = document["targets"]
            .as_array_mut()
            .expect("the shared directory template lists targets");
        if let Some(reader) = targets.first_mut() {
            reader["hostnames"] = json!([hostname()]);
            reader["services"] = json!([]);
        }
        if let Some(authority) = targets.last_mut() {
            authority["name"] = json!(AUTHORITY);
            authority["ssh"] = json!(AUTHORITY_SSH);
            authority["services"] = json!([]);
        }
        document["service_directory"]["authority"] =
            json!({"target": AUTHORITY, "command": AUTHORITY_STADO});
        document["service_directory"]["services"] = json!({});
        std::fs::write(
            self.root.join(".stado/cache/registry-last-good.json"),
            serde_json::to_vec_pretty(&document).expect("the copy serializes"),
        )
        .expect("write the last-known-good copy");
        let read_at = chrono::DateTime::<chrono::Utc>::UNIX_EPOCH.to_rfc3339();
        std::fs::write(
            self.root.join(".stado/cache/registry-last-good.meta.json"),
            serde_json::to_vec(&json!({"read_at": read_at, "generation": "example"}))
                .expect("the copy's metadata serializes"),
        )
        .expect("write the last-known-good metadata");
    }

    fn registry_self(&self, store: &str) -> Output {
        let token = self.root.join("object-api.token");
        std::fs::write(&token, "lost-refusal-probe").expect("write the store token");
        let output = stado_wait::output(
            self.stado()
                .args(["registry", "self", "--name-only"])
                .env("WC_STORAGE_BACKEND", "stado")
                .env("WC_STADO_STORAGE_URL", format!("http://{store}"))
                .env("WC_STADO_STORAGE_TOKEN_FILE", &token),
        )
        .expect("run the real stado");
        std::fs::write(self.root.join("registry-self.stderr"), &output.stderr)
            .expect("keep the command's stderr beside the run");
        output
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.root) {
            eprintln!(
                "the run directory {} was not removed: {error}",
                self.root.display()
            );
        }
    }
}

fn hostname() -> String {
    let output =
        stado_wait::output(&mut Command::new("hostname")).expect("run this machine's hostname");
    String::from_utf8(output.stdout)
        .expect("the hostname is UTF-8")
        .trim()
        .to_lowercase()
}

#[test]
fn a_refusal_the_store_will_not_record_never_replaces_the_reads_own_error() {
    let run = Run::new();
    run.write_last_good();
    let store = run.resetting_store();
    let output = run.registry_self(&store);
    let said = String::from_utf8_lossy(&output.stderr).into_owned();

    assert!(
        !output.status.success(),
        "a read whose authority is unreachable is refused, not answered: {said}"
    );
    assert!(
        said.contains(SSH_FAILED),
        "the read's own error stays the answer: {said}"
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "",
        "no reader name is printed for a read that was refused ({READER} is the copy's reader)"
    );
}
