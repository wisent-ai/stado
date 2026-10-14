//! A local installation decides whether a receipt's host is this machine
//! from this machine's own registry record, never from the registry
//! authority: `stado registry self --local`, which every installation asks,
//! answers at once while the authority accepts connections and answers
//! nothing, and refuses naming the record when this machine keeps none.
//!
//! The real `stado` runs with an isolated `HOME` under the checkout's build
//! directory. Its last-known-good registry copy names, as the registry
//! authority, a loopback listener this test owns that accepts every
//! connection and never speaks; the assertions read the listener's accepted
//! connections, the exit status and the exact sentences. The test holds no
//! clock: an answer that waited on the authority would never come.
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

const READER: &str = "marker-host";
const AUTHORITY: &str = "other-host";
const AUTHORITY_STADO: &str = "/usr/local/bin/stado";

struct Run {
    root: PathBuf,
    /// The connections the authority accepted, kept open so it stays silent.
    held: Arc<Mutex<Vec<TcpStream>>>,
}

impl Run {
    fn new(case: &str) -> Self {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("install-local-identity")
            .join(format!("{}-{case}", std::process::id()));
        std::fs::create_dir_all(root.join(".stado/cache")).expect("create the run directory");
        std::fs::create_dir_all(root.join("tmp")).expect("create the run's scratch directory");
        Self {
            root,
            held: Arc::default(),
        }
    }

    /// The real stado with nothing of this account's configuration.
    fn stado(&self, arguments: &[&str]) -> Output {
        let output = Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(arguments)
            .current_dir(&self.root)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").expect("PATH is set"))
            .env("HOME", &self.root)
            .env("STADO_CONFIG", self.root.join("no-config.toml"))
            .env("TMPDIR", self.root.join("tmp"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("run the real stado");
        let record = json!({
            "arguments": arguments,
            "exit_status": output.status.to_string(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
            "authority_connections": self.held.lock().unwrap().len(),
        });
        std::fs::write(
            self.root
                .join(format!("{}.json", arguments.join("_").replace('-', ""))),
            serde_json::to_vec_pretty(&record).unwrap(),
        )
        .expect("keep the command's record beside the run");
        output
    }

    /// An authority that accepts and never speaks, on a port the product
    /// hands out now.
    fn silent_authority(&self) -> String {
        let printed = self.stado(&["host", "local", "port"]);
        let port: u16 = String::from_utf8_lossy(&printed.stdout)
            .trim()
            .parse()
            .expect("stado host local port prints a port");
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))
            .expect("bind the loopback port this machine handed out");
        let address = listener.local_addr().unwrap().to_string();
        let held = Arc::clone(&self.held);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                held.lock().unwrap().push(stream);
            }
        });
        address
    }

    /// The local record: the shared directory template, the reader carrying
    /// `reader_hostnames`, the silent listener as the registry authority.
    fn write_record(&self, authority_ssh: &str, reader_hostnames: Value) -> PathBuf {
        let mut document: Value =
            serde_json::from_str(include_str!("../service_directory/registry.json")).unwrap();
        let targets = document["targets"].as_array_mut().unwrap();
        let reader = targets
            .first_mut()
            .expect("the template declares the reader first");
        assert_eq!(reader["name"], READER, "the template's reader");
        reader["hostnames"] = reader_hostnames;
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
        let copy = self.root.join(".stado/cache/registry-last-good.json");
        std::fs::write(&copy, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
        std::fs::write(
            self.root.join(".stado/cache/registry-last-good.meta.json"),
            serde_json::to_vec(
                &json!({"read_at": "1970-01-01T00:00:00Z", "generation": "example"}),
            )
            .unwrap(),
        )
        .unwrap();
        copy
    }
}

/// This machine's name as the `hostname` command spells it.
fn system_hostname() -> String {
    let output = Command::new("hostname").output().unwrap();
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

/// The same name as a registry declares it.
fn hostname() -> String {
    system_hostname().to_lowercase()
}

fn said(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn this_machine_is_named_from_its_own_record_while_the_authority_is_silent() {
    let run = Run::new("named");
    let authority = run.silent_authority();
    let copy = run.write_record(&authority, json!([hostname()]));

    let answer = run.stado(&["registry", "self", "--local", "--json"]);
    assert!(
        answer.status.success(),
        "registry self --local: {}",
        said(&answer)
    );
    let printed: Value = serde_json::from_slice(&answer.stdout).expect("one JSON object");
    assert_eq!(printed["name"], READER, "{printed}");
    assert_eq!(printed["source"], copy.display().to_string(), "{printed}");

    let bare = run.stado(&["registry", "self", "--local", "--name-only"]);
    assert!(
        bare.status.success(),
        "registry self --local --name-only: {}",
        said(&bare)
    );
    assert_eq!(String::from_utf8_lossy(&bare.stdout).trim(), READER);

    assert!(
        run.held.lock().unwrap().is_empty(),
        "the registry authority was never asked"
    );
}

#[test]
fn a_machine_its_record_does_not_carry_is_answered_and_refused_by_name() {
    let run = Run::new("absent");
    let authority = run.silent_authority();
    let copy = run.write_record(&authority, json!(["example-elsewhere"]));

    let answer = run.stado(&["registry", "self", "--local", "--json"]);
    assert!(
        answer.status.success(),
        "registry self --local --json: {}",
        said(&answer)
    );
    let printed: Value = serde_json::from_slice(&answer.stdout).expect("one JSON object");
    assert_eq!(printed["name"], Value::Null, "{printed}");
    assert_eq!(printed["hostname"], system_hostname(), "{printed}");

    let refused = run.stado(&["registry", "self", "--local", "--name-only"]);
    assert!(
        !refused.status.success(),
        "a machine the record does not carry is refused"
    );
    assert!(
        said(&refused).contains(&format!(
            "host {} is not in {}",
            system_hostname(),
            copy.display()
        )),
        "the refusal names this machine and the record read: {}",
        said(&refused)
    );
    assert!(
        run.held.lock().unwrap().is_empty(),
        "the registry authority was never asked"
    );
}

#[test]
fn a_machine_with_no_record_is_refused_naming_the_record() {
    let run = Run::new("none");
    let refused = run.stado(&["registry", "self", "--local", "--json"]);
    assert!(
        !refused.status.success(),
        "no record is a refusal, not an answer"
    );
    let copy = run.root.join(".stado/cache/registry-last-good.json");
    assert!(
        said(&refused).contains(&format!(
            "this machine keeps no usable local registry record: {} and its .meta.json sidecar \
             are not both there and readable; this host's resolver writes them on every refresh, \
             and `stado registry self` without --local writes them once the registry authority \
             answers",
            copy.display()
        )),
        "the refusal names the record and what writes it: {}",
        said(&refused)
    );
}
