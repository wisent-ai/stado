//! One isolated fleet per case, and it is THIS machine.
//!
//! The target's `name` and `hostnames` come from this machine's own kernel
//! host name (`/bin/hostname`, trimmed and lower-cased, because the registry
//! spells a host normalized). That is what makes
//! `host_channel::target_is_this_host` true, so every command below takes the
//! product's current-host path — the branch that runs the program here rather
//! than opening an ssh connection — and executes this machine's real
//! `/bin/launchctl`, `/usr/bin/plutil` and `/usr/libexec/PlistBuddy`.
//!
//! `ssh` names this account on this machine's loopback because
//! `host_recovery::resolve_target` refuses a target that declares no
//! connection path at all. It is never dialled: the identity words match this
//! host, so the pass runs here.
//!
//! The target declares no `account_ref`, which is the honest state of a fleet
//! with no credential broker: a privileged lifecycle step has no host-account
//! password to read, and `service stop` of a system LaunchDaemon says so.
//!
//! Isolation: a tempdir per case holding `HOME` and the local storage root,
//! `STADO_CONFIG` at a path that does not exist, and `env_clear` on every run
//! — the operator's registry, vault and config are unreachable from here.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};

use super::unit::{plist_text, Unit, BEACON};

/// The only PATH the product is given: this machine's own system
/// directories, so every tool it runs is the real one.
pub const SYSTEM_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

pub struct Fleet {
    dir: tempfile::TempDir,
    pub home: PathBuf,
    pub storage: PathBuf,
    /// This machine's own host name, lower-cased and without the mDNS
    /// suffix: the registry target's name, and the host every report names.
    pub target: String,
}

impl Fleet {
    /// A fleet of one machine that declares no services of its own, so the
    /// managed beacon resolves to the path the product's fixed list carries.
    pub fn new() -> Self {
        let fleet = Self::empty();
        fleet.write(json!([]));
        fleet
    }

    /// Re-declare this fleet's one target with a single `services[]` record.
    /// A case builds the unit first, because the record has to name the path
    /// the unit really has.
    pub fn declare(&self, record: Value) {
        self.write(json!([record]));
    }

    fn empty() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("stado-recovery-real-")
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

    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    /// This login's launchd agent directory inside the tempdir. The remote
    /// program derives `$HOME/Library/LaunchAgents/<label>.plist` from the
    /// `HOME` the command was given, so this is where it looks.
    pub fn agents(&self) -> PathBuf {
        self.home.join("Library/LaunchAgents")
    }

    pub fn registry_path(&self) -> PathBuf {
        self.storage.join("registry.json")
    }

    fn write(&self, services: Value) {
        let document = json!({
            "schema_version": stado::targets::REGISTRY_SCHEMA_VERSION,
            "targets": [{
                "name": self.target,
                "kind": "local",
                "ssh": format!("{}@127.0.0.1", account()),
                "release_platform": release_platform(),
                "hostnames": [normalized_hostname()],
                "role": "interactive",
                "services": services,
            }],
            "coordinators": [],
        });
        std::fs::write(
            self.registry_path(),
            format!(
                "{}\n",
                serde_json::to_string_pretty(&document).expect("a registry document")
            ),
        )
        .expect("seed the isolated registry");
    }

    /// The persisted document as bytes, for a refusal that must leave it
    /// untouched.
    pub fn registry_bytes(&self) -> String {
        std::fs::read_to_string(self.registry_path()).expect("the registry blob exists")
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

    /// Write the managed beacon's unit file where the pass looks for it,
    /// carrying the declared environment the case is about. The label is the
    /// product's own fixed one, so the guard is what keeps it from outliving
    /// the case.
    pub fn install_beacon(&self, environment: &[(&str, &str)]) -> Unit {
        let unit = Unit::claim(self, BEACON);
        std::fs::write(&unit.plist, plist_text(BEACON, environment)).expect("write the unit file");
        unit
    }
}

/// One `services[]` record, in the spelling `ManagedService::from_record`
/// reads: the label is the unit id a command addresses, and the path is what
/// decides which launchd domain the unit belongs to.
pub fn launchd_record(label: &str, path: &str) -> Value {
    json!({
        "name": label,
        "unit": "",
        "label": label,
        "path": path,
        "kind": "launchd",
        "managed_since": "2026-09-08T00:00:00Z",
    })
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

/// The account running this test, from the real `id -un`.
pub fn account() -> String {
    let output = Command::new("/usr/bin/id")
        .arg("-un")
        .output()
        .expect("/usr/bin/id did not run");
    assert!(output.status.success(), "/usr/bin/id -un failed");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// This machine's host name, from the kernel, spelled the way the registry
/// accepts it: trimmed and lower-cased.
pub fn normalized_hostname() -> String {
    let output = Command::new("/bin/hostname")
        .output()
        .expect("/bin/hostname did not run");
    assert!(output.status.success(), "/bin/hostname failed");
    let name = String::from_utf8_lossy(&output.stdout)
        .trim()
        .to_lowercase()
        .trim_end_matches('.')
        .to_string();
    assert!(!name.is_empty(), "this machine reports no host name");
    name
}

/// The registry target name: the same name without the mDNS suffix, which is
/// what this machine answers to on its own LAN and what `hostname -s` prints
/// — the word the recovery pass compares its identity against.
pub fn local_host_name() -> String {
    normalized_hostname().trim_end_matches(".local").to_string()
}

fn release_platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("macos", _) => "darwin-amd64",
        (_, "aarch64") => "linux-arm64",
        _ => "linux-amd64",
    }
}
