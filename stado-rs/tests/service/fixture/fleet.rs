//! One isolated fleet per case, and it is THIS machine.
//!
//! The target's `name` is this host's own name lower-cased with the mDNS
//! suffix dropped, and its `hostnames` carry the normalized name the registry
//! write contract accepts (`registry.targets[0].hostnames[0]: must be
//! normalized as '<name>'` is the refusal a raw `hostname` produces). That is
//! what makes `host_channel::target_is_this_host` true, so every
//! `stado service ...` below takes the product's current-host path — the
//! branch that runs `/bin/bash -s` here rather than opening an ssh
//! connection — and executes this machine's own `/bin/launchctl`,
//! `/usr/bin/plutil` and `/usr/libexec/PlistBuddy`.
//!
//! [`Fleet::lifecycle`] declares NO ssh destination at all, which is the
//! honest shape for a fleet of one machine and the reason nothing in the
//! lifecycle cases can reach a remote host even by mistake.
//! [`Fleet::directory`] adds a service directory, and the directory contract
//! demands the authority target declare a connection path
//! (`registry.service_directory.authority.target: must declare an SSH
//! connection path`), so that one target names this account on this machine's
//! loopback. `declare` touches no host, so it is never dialled.
//!
//! Isolation: a fresh tempdir per case holding `HOME` and the local storage
//! root, `STADO_CONFIG` at a path that does not exist, and `env_clear` on
//! every run — the operator's registry, vault, config and launchd units are
//! unreachable from here.

use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::{json, Value};

/// The only PATH the product is given: this machine's own system
/// directories, so every tool it runs is the real one.
pub const SYSTEM_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

/// The generation an isolated service directory starts at. The contract
/// refuses `0` (`registry.service_directory.generation: must be positive`),
/// and `declare` must advance whatever it is handed.
pub const FIRST_GENERATION: u64 = 1;

/// A declaration's digest is 64 lowercase hex characters; the validator says
/// so, and `declaration.rs` proves it.
pub const SHA256_HEX_LEN: usize = 64;

/// The loopback endpoint a declaration's `port` shorthand expands to. Nothing
/// listens there: every assertion is about the endpoint travelling from the
/// declaration into the persisted directory.
pub const PORT: u64 = 48231;

pub struct Fleet {
    dir: tempfile::TempDir,
    pub home: PathBuf,
    pub storage: PathBuf,
    /// This machine's own host name, lower-cased. It is the registry target's
    /// name, so it is also the host every report names.
    pub target: String,
}

impl Fleet {
    /// A fleet of one machine with no service directory and no ssh
    /// destination: the fixture every launchd lifecycle case drives.
    pub fn lifecycle() -> Self {
        let fleet = Self::empty();
        fleet.write(&fleet.document(Value::Null, None));
        fleet
    }

    /// The same machine plus a service directory whose authority it is.
    pub fn directory() -> Self {
        let fleet = Self::empty();
        fleet.write(&fleet.document(
            json!(format!("{}@127.0.0.1", account())),
            Some(json!({
                "authority": {
                    "target": fleet.target,
                    "command": env!("CARGO_BIN_EXE_stado"),
                },
                "generation": FIRST_GENERATION,
                "services": {},
            })),
        ));
        fleet
    }

    fn empty() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("stado-service-real-")
            .tempdir()
            .expect("an isolated fleet root");
        let root = dir.path().to_path_buf();
        let fleet = Self {
            home: root.join("home"),
            storage: root.join("storage"),
            dir,
            target: local_host_name(),
        };
        for directory in [
            fleet.agents(),
            fleet.storage.clone(),
            fleet.root().join("tmp"),
        ] {
            std::fs::create_dir_all(&directory).expect("an isolated directory");
        }
        fleet
    }

    pub fn root(&self) -> &std::path::Path {
        self.dir.path()
    }

    /// This login's launchd agent directory, inside the tempdir. The remote
    /// prelude derives `$HOME/Library/LaunchAgents/<label>.plist` from the
    /// `HOME` handed to the command, so this is where the product installs.
    pub fn agents(&self) -> PathBuf {
        self.home.join("Library/LaunchAgents")
    }

    /// Where `service logs` reads a launchd unit's tail from.
    pub fn logs(&self) -> PathBuf {
        self.home.join(".stado/logs")
    }

    pub fn registry_path(&self) -> PathBuf {
        self.storage.join("registry.json")
    }

    fn document(&self, ssh: Value, directory: Option<Value>) -> Value {
        let mut document = json!({
            "schema_version": stado::targets::REGISTRY_SCHEMA_VERSION,
            "targets": [{
                "name": self.target,
                "kind": "local",
                "ssh": ssh,
                "release_platform": release_platform(),
                "hostnames": [normalized_hostname()],
                "role": "interactive",
                "services": [],
            }],
            "coordinators": [],
        });
        if let Some(directory) = directory {
            document["service_directory"] = directory;
        }
        document
    }

    pub fn write(&self, document: &Value) {
        std::fs::write(
            self.registry_path(),
            format!(
                "{}\n",
                serde_json::to_string_pretty(document).expect("a registry document")
            ),
        )
        .expect("seed the isolated registry");
    }

    /// The canonical document exactly as it is persisted right now.
    pub fn registry(&self) -> Value {
        serde_json::from_str(&self.registry_bytes()).expect("the persisted registry is JSON")
    }

    /// The persisted document as bytes, for a refusal that must leave it
    /// untouched.
    pub fn registry_bytes(&self) -> String {
        std::fs::read_to_string(self.registry_path()).expect("the registry blob exists")
    }

    /// The `services` array this machine's target declares.
    pub fn declared(&self) -> Vec<Value> {
        self.registry()["targets"][0]["services"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    /// The built binary, with nothing of the operator's environment left.
    pub fn stado(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(args)
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", SYSTEM_PATH)
            .env("TMPDIR", self.root().join("tmp"))
            .env("STADO_CONFIG", self.root().join("no-such-config.json"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &self.storage)
            .env("NO_COLOR", "1")
            .output()
            .expect("the built stado binary did not start")
    }
}

/// Both streams, because a refusal reaches stderr and a report reaches
/// stdout, and a case that read only one would miss the other entirely.
pub fn said(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

pub fn json_stdout(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("stado did not answer with JSON: {error}\n{}", said(output)))
}

/// This machine's host name, through the same call the product resolves its
/// own target with.
pub fn hostname() -> String {
    let name = stado::providers::vast::system_hostname();
    assert!(!name.is_empty(), "this machine reports no hostname");
    name
}

/// The registry-normalized spelling: trimmed, lower-cased, no trailing dot.
/// A raw `hostname` here is refused by the write contract.
pub fn normalized_hostname() -> String {
    stado::targets::normalize_hostname(&hostname())
}

/// The registry target name: the normalized host name without the mDNS
/// suffix, which is what this machine answers to on its own LAN.
pub fn local_host_name() -> String {
    let name = normalized_hostname();
    name.trim_end_matches(".local").to_string()
}

/// The account running this test, from the real `id -un`.
pub fn account() -> String {
    let output = Command::new("/usr/bin/id")
        .arg("-un")
        .output()
        .expect("/usr/bin/id did not run");
    assert!(output.status.success(), "/usr/bin/id -un failed");
    String::from_utf8(output.stdout)
        .expect("an account name is UTF-8")
        .trim()
        .to_string()
}

fn release_platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("macos", _) => "darwin-amd64",
        (_, "aarch64") => "linux-arm64",
        _ => "linux-amd64",
    }
}
