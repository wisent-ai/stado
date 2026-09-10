//! Real evidence for resolver host and endpoint resolution.
//!
//! Every case drives the built `stado` binary against an isolated local
//! registry whose targets are declared here and whose one *current* host is
//! the machine running the test: `hostnames` carries this kernel's own name,
//! lower-cased the way registry-v2 requires, so `host_channel` matches it and
//! the product takes its current-host path. Nothing in this area substitutes
//! an executable, and `PATH` is the system one, so no stand-in can be found
//! ahead of a real tool. The single `ssh` field the service-directory
//! contract demands of an authority target
//! (`registry.service_directory.authority.target: must declare an SSH
//! connection path`) names this machine's own loopback and is never dialled:
//! every readiness answer below is produced with the authority read taking
//! the local-store branch, which the report itself states as
//! `authority.source == "local"`.
//!
//! What is proved, and where:
//!
//! * a real `resolver serve` on this machine, its published state, and the
//!   readiness verdicts derived from it — `readiness.rs`;
//! * resolution of a declared service through the CLI, through the running
//!   resolver's API, and into the forward marker on disk — `resolution.rs`;
//! * the refusals for an unknown host, an ambiguous identity, a malformed
//!   declaration and a host declared for another platform — `declaration.rs`.
//!
//! Assertions read state: the published state file, the marker on disk, the
//! exit code, the socket that is or is not open. Stdout corroborates. Every
//! refusal sentence is copied from a live run of this fixture.

mod answers;
mod declaration;
mod fixture;
mod published;
mod readiness;
mod readiness_probe;
mod resolution;
mod resolution_refusals;

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::Value;

/// The document contract's own schema field, and the identifiers this area
/// declares. All four are contract: the directory route names a
/// `managed_service` its host must declare, and the adapter must name a
/// consumer the route authorizes, or `resolver serve` refuses the policy.
pub const REGISTRY_SCHEMA_VERSION: u64 = 2;
pub const TARGET: &str = "resolver-real-host";
pub const SERVICE: &str = "stado-object-api";
pub const CONSUMER: &str = "stado-local-agent";

/// The system tools only. A test cannot put anything of its own in front of
/// what the product executes.
const SYSTEM_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

/// This kernel's name, normalized as the registry requires.
///
/// `resolver serve` and `resolver status` identify their own host by asking
/// the operating system, and nothing overrides that from the environment, so
/// the fixture has to name the machine the test runs on. Lower-cased because
/// an alias that is not normalized is refused by the document contract with
/// `must be normalized as '...'`, and macOS answers `hostname` with capitals.
pub fn hostname() -> String {
    let named = Command::new("hostname")
        .env_clear()
        .env("PATH", SYSTEM_PATH)
        .output()
        .expect("blocked: the real hostname executable could not start");
    assert!(named.status.success(), "the real hostname read failed");
    let name = String::from_utf8(named.stdout)
        .expect("the kernel hostname is UTF-8")
        .trim()
        .to_ascii_lowercase();
    assert!(!name.is_empty(), "this machine answers to no hostname");
    name
}

/// The release platform this machine actually is, in the product's spelling.
pub fn platform() -> String {
    format!(
        "{}-{}",
        match std::env::consts::OS {
            "macos" => "darwin",
            other => other,
        },
        match std::env::consts::ARCH {
            "aarch64" => "arm64",
            other => other,
        }
    )
}

/// A loopback port nothing holds: bound to learn the number, then released.
pub fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("loopback bind");
    listener.local_addr().expect("bound address").port()
}

/// A loopback port something holds for as long as the listener lives.
pub fn held_port() -> (std::net::TcpListener, u16) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("loopback bind");
    let port = listener.local_addr().expect("bound address").port();
    (listener, port)
}

/// One isolated host: a fresh temp root, local storage carrying one registry
/// document, a config path that does not exist, and a HOME inside the root
/// because two things the resolver touches are HOME-derived — the state file
/// `serve` publishes and the last-known-good registry cache. Leaving the real
/// HOME reachable would have this area overwrite the operator's cached
/// registry with a fixture.
pub struct Host {
    pub root: tempfile::TempDir,
    pub home: PathBuf,
    pub storage: PathBuf,
}

impl Host {
    pub fn new(document: &Value) -> Self {
        let root = tempfile::Builder::new()
            .prefix("stado-resolver-real-")
            .tempdir()
            .expect("an isolated resolver root");
        let home = root.path().join("home");
        let storage = root.path().join("storage");
        for directory in [&home, &storage] {
            fs::create_dir_all(directory).expect("an isolated resolver directory");
        }
        let host = Self {
            root,
            home,
            storage,
        };
        host.write_registry(document);
        host
    }

    pub fn write_registry(&self, document: &Value) {
        fs::write(
            self.registry_path(),
            format!("{}\n", serde_json::to_string_pretty(document).unwrap()),
        )
        .expect("seed the isolated registry");
    }

    pub fn registry_path(&self) -> PathBuf {
        self.storage.join("registry.json")
    }

    /// The built binary, with nothing of the operator's environment
    /// reachable: not their storage, not their config, not their credentials.
    pub fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .args(args)
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", SYSTEM_PATH)
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &self.storage)
            .env("STADO_CONFIG", self.root.path().join("no-such-config.json"));
        command
    }

    pub fn stado(&self, args: &[&str]) -> Output {
        self.command(args)
            .output()
            .expect("the built stado binary runs")
    }

    /// Where a `resolver serve` process publishes what it holds.
    pub fn state_path(&self) -> PathBuf {
        self.home.join(".stado").join("resolver-state.json")
    }

    /// That file as it stands right now, or `None` while no resolver has
    /// written one.
    pub fn published_state(&self) -> Option<Value> {
        let raw = fs::read_to_string(self.state_path()).ok()?;
        serde_json::from_str(&raw).ok()
    }

    /// Where `route open --local` writes the resolved endpoint.
    pub fn marker(&self, service: &str) -> PathBuf {
        self.home
            .join(".stado/forwards")
            .join(format!("{service}.local"))
    }
}

/// Both streams: a refusal reaches stderr and a report reaches stdout, and a
/// failure message that read only one would hide the other.
pub fn said(output: &Output) -> String {
    format!(
        "exit={:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// A `--json` report, parsed.
pub fn report(output: &Output) -> Value {
    serde_json::from_str(&stdout(output))
        .unwrap_or_else(|error| panic!("expected one JSON object: {error}\n{}", said(output)))
}

/// What the repository ships as a registry, and what it must not ship.
///
/// `d3dbabdf fix: ship an empty public registry seed` (2026-08-20) replaced
/// one operator's hosts, launchd paths and ports with `"targets": []`,
/// because a public repository has no business carrying them. What is
/// checkable from a public checkout is that the seed is a valid registry-v2
/// document by the product's own validator and names no host, so a fresh
/// install cannot silently adopt somebody else's declaration.
#[test]
fn the_shipped_registry_seed_names_no_host_and_still_validates() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("data")
        .join("registry.json");
    let seed: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(seed["schema_version"], REGISTRY_SCHEMA_VERSION);
    assert_eq!(
        seed["targets"],
        serde_json::json!([]),
        "the public seed carries a host"
    );
    assert_eq!(
        seed["coordinators"],
        serde_json::json!([]),
        "the public seed carries a coordinator"
    );

    let host = Host::new(&seed);
    let answer = host.stado(&["registry", "validate", path.to_str().unwrap()]);
    assert!(
        answer.status.success(),
        "the shipped seed does not validate: {}",
        said(&answer)
    );
    assert!(
        stdout(&answer).contains("valid registry: "),
        "got: {}",
        said(&answer)
    );

    // And a host that installs it holds no declaration to serve.
    let answer = host.stado(&["resolver", "status", "--target", "operator-host"]);
    assert_eq!(answer.status.code(), Some(1), "{}", said(&answer));
    assert!(
        stderr(&answer).contains("Error: resolver target \"operator-host\" is not registered"),
        "got: {}",
        said(&answer)
    );
}
