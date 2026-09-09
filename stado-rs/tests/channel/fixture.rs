//! The isolated canonical registry naming the machine that runs this area,
//! the product invocation every case drives, and the readers the cases assert
//! the persisted document through.
//!
//! Split out of `main.rs` so every file in this area stays inside the three
//! hundred line limit this repository enforces on itself.

use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use serde_json::{json, Value};

/// The declared origin name, the path it publishes and the publication that
/// carries it, spelled the way `stado web origin declare` and
/// `/docs/channels` spell them. Declared names copied from a live run, not
/// constants, config or tuning: nothing here changes what the product does.
pub const ORIGIN: &str = "release-object";
pub const PUBLISHED_PATH: &str = "/api/release/object";
pub const FUNNEL: &str = "tailscale-funnel";

/// The registry name for the one target these cases declare — this machine.
/// It is a name rather than one of this host's own host names because the
/// registry refuses one host identity declared twice inside a target, and
/// `hostnames` is where the kernel's answer belongs.
pub const TARGET: &str = "current-host-channel";

/// The system directories the product resolves its own tools out of. Nothing
/// belonging to this test is on it, so nothing a case wrote can be discovered
/// as if it were a system tool.
pub const SYSTEM_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

/// The machine running this test, as the kernel reports it, lowercased: the
/// registry refuses a declared host name that is not normalized.
pub fn hostname() -> String {
    let output = Command::new("/bin/hostname")
        .output()
        .expect("the operating system reports its host name");
    let name = String::from_utf8_lossy(&output.stdout)
        .trim()
        .to_lowercase();
    assert!(
        name.contains('.'),
        "this machine's kernel host name carries no domain label, so it cannot \
         be a public origin's subject: {name:?}"
    );
    name
}

/// The first label of that name. This machine answers to it as well, which is
/// how the registry matches a target to the host it is standing on.
pub fn short_hostname() -> String {
    hostname().split('.').next().unwrap_or_default().to_string()
}

/// The login running this test: the user half of a real control route back to
/// this same machine.
pub fn login() -> String {
    let output = Command::new("/usr/bin/id")
        .arg("-un")
        .output()
        .expect("the operating system reports the login running the test");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

pub fn release_platform() -> &'static str {
    if std::env::consts::OS == "macos" {
        "darwin-arm64"
    } else {
        "linux-amd64"
    }
}

fn target_row(hostnames: Value, ssh: Value) -> Value {
    json!({
        "name": TARGET,
        "kind": "local",
        "ssh": ssh,
        "release_platform": release_platform(),
        "hostnames": hostnames,
        "services": [],
    })
}

/// A registry declaring this machine and nothing else, with no control route:
/// reaching it does not involve the network, so the product takes its
/// current-host path.
pub fn current_host_registry() -> Value {
    json!({
        "schema_version": stado::targets::REGISTRY_SCHEMA_VERSION,
        "coordinators": [],
        "targets": [target_row(json!([hostname()]), Value::Null)],
    })
}

/// The same machine, declaring a real SSH control route to itself. The route's
/// host is this machine's own name, so a public origin declared on that name
/// is one derived from a host-control destination — what `/docs/channels`
/// forbids. `hostnames` carries the short name here because the registry
/// refuses the same host identity declared twice in one target.
pub fn control_route_registry() -> Value {
    let route = format!("{}@{}", login(), hostname());
    json!({
        "schema_version": stado::targets::REGISTRY_SCHEMA_VERSION,
        "coordinators": [],
        "targets": [target_row(json!([short_hostname()]), json!(route))],
    })
}

/// One declaration row, as the `public_origins` key carries it.
pub fn origin_row(hostname: &str, target: &str, upstream: &str) -> Value {
    json!([{
        "name": ORIGIN,
        "hostname": hostname,
        "target": target,
        "publication": FUNNEL,
        "upstream": upstream,
        "paths": [PUBLISHED_PATH],
    }])
}

pub struct Fixture {
    root: tempfile::TempDir,
}

impl Fixture {
    pub fn new() -> Self {
        let fixture = Self {
            root: tempfile::tempdir().expect("an isolated storage root"),
        };
        std::fs::create_dir_all(fixture.store()).expect("the canonical store directory exists");
        std::fs::create_dir_all(fixture.home()).expect("the isolated home exists");
        fixture.seed(&current_host_registry());
        fixture
    }

    pub fn store(&self) -> PathBuf {
        self.root.path().join("store")
    }

    pub fn home(&self) -> PathBuf {
        self.root.path().join("home")
    }

    pub fn registry_path(&self) -> PathBuf {
        self.store().join("registry.json")
    }

    /// Put a document in place without going through a command, for the cases
    /// whose subject is what a command does to it afterwards.
    pub fn seed(&self, document: &Value) {
        std::fs::write(self.registry_path(), body(document)).expect("seed the isolated registry");
    }

    /// The product, with the canonical store on this disk and nothing from the
    /// surrounding environment: no inherited API origin, profile directory or
    /// credential can decide what a case reads.
    pub fn stado(&self, args: &[&str]) -> Output {
        self.stado_with(&[], args)
    }

    pub fn stado_with(&self, extra: &[(&str, &str)], args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .env_clear()
            .env("HOME", self.home())
            .env("PATH", SYSTEM_PATH)
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.store())
            .env("STADO_CONFIG", self.root.path().join("no-such-config.json"));
        for (name, value) in extra {
            command.env(name, value);
        }
        command
            .args(args)
            .stdin(Stdio::null())
            .output()
            .expect("the built stado binary runs")
    }

    /// Write a document into the fixture and hand it to a registry command.
    fn with_document(&self, verb: &str, document: &Value) -> Output {
        let path = self.home().join(format!("{verb}.json"));
        std::fs::write(&path, body(document)).expect("the candidate document is written");
        self.stado(&["registry", verb, path.to_str().expect("a UTF-8 path")])
    }

    pub fn push(&self, document: &Value) -> Output {
        self.with_document("push", document)
    }

    pub fn validate(&self, document: &Value) -> Output {
        self.with_document("validate", document)
    }

    /// The canonical document exactly as it stands on this disk, which is
    /// where a declaration has to land to mean anything.
    pub fn registry_bytes(&self) -> Vec<u8> {
        std::fs::read(self.registry_path()).expect("read the isolated registry")
    }

    pub fn registry(&self) -> Value {
        serde_json::from_slice(&self.registry_bytes()).expect("the registry stays valid JSON")
    }

    /// The declarations the persisted document carries, or `None` when it
    /// carries no `public_origins` key at all — which is what a fleet that
    /// publishes nothing publicly looks like on disk.
    pub fn persisted_origins(&self) -> Option<Value> {
        self.registry()
            .as_object()
            .expect("the registry document is an object")
            .get("public_origins")
            .cloned()
    }
}

pub fn body(document: &Value) -> Vec<u8> {
    serde_json::to_vec_pretty(document).expect("serialize the document")
}

pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

pub fn report(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout was not one JSON report: {error}\nstdout={}\nstderr={}",
            stdout(output),
            stderr(output)
        )
    })
}

/// The one row a report carries, so a case reads a fact and not an index.
pub fn only_row(rows: &Value) -> &Value {
    let rows = rows.as_array().expect("the report is an array of rows");
    assert_eq!(
        rows.len(),
        usize::from(true),
        "exactly one declared origin was expected: {rows:?}"
    );
    &rows[0]
}
