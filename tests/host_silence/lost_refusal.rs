//! A reader refusal whose record the fleet store refuses is said on stderr
//! with the host, the reader, the reason and the store's own cause, and the
//! read's own error stays the answer; because the dedupe marks a refusal
//! recorded only once its record is written, the identical refusal raised
//! again is written again instead of being dropped as already recorded.
//!
//! The real `stado registry self` runs with an isolated `HOME` under the
//! checkout's build directory and a last-known-good registry copy that names
//! this machine as the reader and a host nobody can reach (`.invalid`) as the
//! registry authority, so the authority read fails over SSH and publishes an
//! `authority_unreachable` refusal about that host. The fleet store the
//! refusal record goes to is a loopback listener this test owns, which resets
//! every connection, so the record cannot be written. The read retries the
//! authority once on a fresh session and raises the same refusal twice.
use std::io::{BufRead, BufReader};
use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{json, Value};
use stado::monitor::host_silence::REASON_AUTHORITY_UNREACHABLE;

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
        let parent = Path::new(env!("CARGO_TARGET_TMPDIR")).join("host-silence-lost-refusal");
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
            .args(["host", "free-port-local"])
            .output()
            .expect("run the real stado");
        let printed = String::from_utf8_lossy(&output.stdout).into_owned();
        match printed.trim().parse() {
            Ok(port) => port,
            Err(error) => panic!("stado host free-port-local printed {printed:?}: {error}"),
        }
    }

    /// The fleet store the refusal record goes to: a loopback listener that
    /// resets every connection after its request line, so no write lands.
    fn resetting_store(&self) -> String {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, self.free_port()))
            .expect("bind the loopback port this machine handed out");
        let address = listener.local_addr().unwrap().to_string();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut request_line = String::new();
                let _ = BufReader::new(&stream).read_line(&mut request_line);
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
            serde_json::from_str(include_str!("../service_directory/registry.json")).unwrap();
        let targets = document["targets"].as_array_mut().unwrap();
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
    }

    fn registry_self(&self, store: &str) -> Output {
        let token = self.root.join("object-api.token");
        std::fs::write(&token, "lost-refusal-probe").unwrap();
        let output = self
            .stado()
            .args(["registry", "self", "--name-only"])
            .env("WC_STORAGE_BACKEND", "stado")
            .env("WC_STADO_STORAGE_URL", format!("http://{store}"))
            .env("WC_STADO_STORAGE_TOKEN_FILE", &token)
            .output()
            .expect("run the real stado");
        std::fs::write(self.root.join("registry-self.stderr"), &output.stderr)
            .expect("keep the command's stderr beside the run");
        output
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
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
fn a_refusal_the_store_will_not_record_is_said_with_its_cause_and_written_again() {
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
    let lost = format!(
        "refusal about {AUTHORITY} ({REASON_AUTHORITY_UNREACHABLE}) was not recorded in the fleet store: "
    );
    let lost_lines: Vec<&str> = said.lines().filter(|line| line.contains(&lost)).collect();
    let Some((first, again)) = lost_lines.split_first() else {
        panic!("a refusal record the store refused is said on stderr with the host and the reason: {said}");
    };
    assert!(
        first.starts_with("stado: the ") && first.contains(" refusal about "),
        "the lost-record line names the reader that refused: {first}"
    );
    let cause = first.split_once("in the fleet store: ").unwrap().1;
    assert!(
        !cause.trim().is_empty(),
        "the store's own cause follows the sentence: {first}"
    );
    assert!(
        !again.is_empty(),
        "the identical refusal raised by the retried authority read is written again, not dropped as already recorded: {said}"
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "",
        "no reader name is printed for a read that was refused ({READER} is the copy's reader)"
    );
}
