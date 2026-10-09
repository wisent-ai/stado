//! The isolated deployment the authority-read journey drives: the real stado
//! binary with `HOME` under the checkout's build directory, a store that is
//! a loopback listener this test never answers (on a port `stado host
//! free-port-local` hands out), the dedicated authority the fixture names,
//! and a report of every command bound to the checkout's revision.
use serde_json::{json, Value};
use std::fs;
use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

/// The reader is the first target of the shared directory template.
pub(crate) const READER: &str = "marker-host";
pub(crate) const STORE_ASKED: &str = "czekam: GET /api/object";

pub(crate) struct Fixture {
    pub(crate) target: String,
    pub(crate) ssh: String,
    pub(crate) stado_command: String,
    ssh_key_file: PathBuf,
    known_hosts_file: PathBuf,
}

impl Fixture {
    pub(crate) fn read() -> Self {
        let path = std::env::var("STADO_REGISTRY_AUTHORITY_FIXTURE").expect(
            "STADO_REGISTRY_AUTHORITY_FIXTURE must name the dedicated authority fixture file",
        );
        let fixture: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(
            fixture["dedicated"],
            json!(true),
            "only an explicitly dedicated host fixture is accepted"
        );
        let field = |name: &str| match fixture[name].as_str() {
            Some(value) => value.to_string(),
            None => panic!("the fixture names {name}"),
        };
        Self {
            target: field("target"),
            ssh: field("ssh"),
            stado_command: field("stado_command"),
            ssh_key_file: PathBuf::from(field("ssh_key_file")),
            known_hosts_file: PathBuf::from(field("known_hosts_file")),
        }
    }
}

/// Take every permission from group and others, the way a bearer file has
/// to be held.
fn owner_only(path: &Path) {
    let status = Command::new("chmod")
        .arg("go-rwx")
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success(), "chmod go-rwx {}", path.display());
}

fn hostname() -> String {
    let output = Command::new("hostname").output().unwrap();
    String::from_utf8(output.stdout)
        .unwrap()
        .trim()
        .to_lowercase()
}

pub(crate) struct Deployment {
    root: PathBuf,
    report: Value,
    /// The "object API" this stado's store names: accepted, never answered.
    store: TcpListener,
}

impl Deployment {
    pub(crate) fn start(fixture: &Fixture) -> Self {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let root = repository
            .join(".build/service-directory-authority-read")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(root.join(".stado/cache")).unwrap();
        fs::create_dir_all(root.join(".ssh")).unwrap();
        fs::create_dir_all(root.join("tmp")).unwrap();
        fs::copy(&fixture.known_hosts_file, root.join(".ssh/known_hosts")).unwrap();
        let token = root.join("object-api.token");
        fs::write(&token, "example-bearer").unwrap();
        owner_only(&token);
        let revision = Command::new("git")
            .current_dir(&repository)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        let printed = Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(["host", "free-port-local"])
            .env("HOME", &root)
            .output()
            .unwrap();
        let printed = String::from_utf8(printed.stdout).unwrap();
        let port: u16 = match printed.trim().parse() {
            Ok(port) => port,
            Err(error) => panic!("stado host free-port-local printed {printed:?}: {error}"),
        };
        let store = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).unwrap();
        store.set_nonblocking(true).unwrap();
        Self {
            root,
            report: json!({
                "source_revision": String::from_utf8_lossy(&revision.stdout).trim(),
                "authority": fixture.target,
                "commands": [],
                "outcome": "failed",
            }),
            store,
        }
    }

    pub(crate) fn last_good(&self) -> PathBuf {
        self.root.join(".stado/cache/registry-last-good.json")
    }

    /// The copy this stado resolves its authority through: the shared
    /// directory template with this machine as the reader (its first
    /// target) and the fixture's host as the authority (its last), and no
    /// service placed anywhere.
    pub(crate) fn write_last_good(&self, fixture: &Fixture) {
        let mut document: Value = serde_json::from_str(include_str!("../registry.json")).unwrap();
        let targets = document["targets"].as_array_mut().unwrap();
        if let Some(reader) = targets.first_mut() {
            reader["hostnames"] = json!([hostname()]);
            reader["services"] = json!([]);
        }
        if let Some(authority) = targets.last_mut() {
            authority["name"] = json!(fixture.target);
            authority["ssh"] = json!(fixture.ssh);
            authority["services"] = json!([]);
        }
        document["service_directory"]["authority"] =
            json!({"target": fixture.target, "command": fixture.stado_command});
        document["service_directory"]["services"] = json!({});
        fs::write(
            self.last_good(),
            serde_json::to_vec_pretty(&document).unwrap(),
        )
        .unwrap();
        fs::write(
            self.root.join(".stado/cache/registry-last-good.meta.json"),
            serde_json::to_vec(&json!({
                "read_at": "1970-01-01T00:00:00Z",
                "generation": "example",
            }))
            .unwrap(),
        )
        .unwrap();
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .args(args)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", &self.root)
            .env("STADO_CONFIG", self.root.join("no-config.json"))
            .env("TMPDIR", self.root.join("tmp"))
            .env("WC_STORAGE_BACKEND", "stado")
            .env(
                "WC_STADO_STORAGE_URL",
                format!("http://{}", self.store.local_addr().unwrap()),
            )
            .env(
                "WC_STADO_STORAGE_TOKEN_FILE",
                self.root.join("object-api.token"),
            )
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    /// Run one command to its end with the fixture's SSH identity.
    pub(crate) fn stado(&mut self, fixture: &Fixture, args: &[&str]) -> Output {
        let output = self
            .command(args)
            .env("STADO_RESOLVER_SSH_KEY_FILE", &fixture.ssh_key_file)
            .output()
            .unwrap();
        self.report["commands"].as_array_mut().unwrap().push(json!({
            "command": args,
            "exit_status": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        }));
        self.save();
        output
    }

    /// Start one command and hand it back running, its stderr piped.
    pub(crate) fn spawn(&mut self, args: &[&str]) -> Child {
        self.command(args).stdout(Stdio::null()).spawn().unwrap()
    }

    pub(crate) fn note(&mut self, command: &str, line: &str) {
        self.report["commands"]
            .as_array_mut()
            .unwrap()
            .push(json!({"command": command, "first_wait": line}));
        self.save();
    }

    /// Whether the never-answering store received a connection.
    pub(crate) fn store_was_asked(&self) -> bool {
        match self.store.accept() {
            Ok(_) => true,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => false,
            Err(error) => panic!("the store listener could not be read: {error}"),
        }
    }

    fn save(&self) {
        fs::write(
            self.root.join("report.json"),
            serde_json::to_vec_pretty(&self.report).unwrap(),
        )
        .unwrap();
    }

    pub(crate) fn finish(&mut self, outcome: &str) {
        self.report["outcome"] = json!(outcome);
        self.save();
        eprintln!("authority read evidence: {}", self.root.display());
    }
}
