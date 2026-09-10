//! The real host these cases run against: this machine, in a registry of its
//! own.
//!
//! The registry names the hostname the kernel reports, so
//! `deploy::host_channel::target_is_this_host` answers yes and the production
//! credential path executes the operating system's real tools here —
//! `/bin/sh`, the real `gpg`, the real `python3` — with no ssh destination in
//! the fixture and no executable substituted. The same shape as
//! `tests/host_exec/main.rs`, on a different subject.
//!
//! Isolation is total. Storage is a local backend inside a fresh tempdir,
//! `STADO_CONFIG` names a path that does not exist, `HOME` and `GNUPGHOME`
//! are inside the tempdir, and the vault is one this fixture creates and
//! initialises with real GnuPG keys of its own. The operator's registry,
//! vault, keyring and configuration are never read and never written.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{json, Value};

use crate::skarbiec::real_skarbiec_binary;

/// The registry name of the isolated entry standing for this machine.
pub const TARGET: &str = "credentials-host-isolated";

/// The owner the fixture's own vault is initialised under. The address is in
/// the reserved `.invalid` TLD, so nothing about it can resolve anywhere.
pub const OWNER: &str = "Stado credentials area <credentials-area@example.invalid>";

/// The paths the product's own credential invocations expect their tools on,
/// fixed here so a real `gpg` and a real `python3` are found identically
/// whatever the shell that started `cargo test` exported.
const SYSTEM_PATH: &str = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";
const REGISTRY_SCHEMA: u32 = 2;
const CONFIG_SCHEMA: u32 = 1;
const OWNER_ONLY_DIRECTORY: u32 = 0o700;

pub struct IsolatedHost {
    root: tempfile::TempDir,
    home: PathBuf,
    storage: PathBuf,
    gnupg: PathBuf,
    vault: PathBuf,
    broker: PathBuf,
}

impl IsolatedHost {
    /// A fresh isolated host holding a real initialised vault.
    ///
    /// `declared` decides only whether the host's own Stado configuration
    /// names that vault in `secrets.skarbiec.vault_file`. The vault exists
    /// either way, so an undeclared host refuses because nothing declares an
    /// authority and not because there is no file to find.
    pub fn new(declared: bool) -> Self {
        let root = tempfile::Builder::new()
            .prefix("credentials-host-")
            .tempdir()
            .expect("create the isolated journey root");
        let home = root.path().join("home");
        let storage = root.path().join("storage");
        let gnupg = root.path().join("gnupg");
        for directory in [
            &home,
            &storage,
            &gnupg,
            &home.join(".stado/bin"),
            &home.join(".config/stado"),
        ] {
            fs::create_dir_all(directory).expect("create an isolated journey directory");
        }
        fs::set_permissions(&gnupg, fs::Permissions::from_mode(OWNER_ONLY_DIRECTORY))
            .expect("keep the isolated GnuPG home owner-only");

        let vault = home.join(".stado/credentials-area.vault.json");
        fs::write(
            storage.join("registry.json"),
            serde_json::to_vec_pretty(&json!({
                "schema_version": REGISTRY_SCHEMA,
                "targets": [{
                    "name": TARGET,
                    "kind": "local",
                    "ssh": null,
                    "release_platform": release_platform(),
                    "hostnames": [hostname()],
                    "services": [],
                }],
                "coordinators": [],
            }))
            .unwrap(),
        )
        .expect("write the isolated registry");

        let mut configuration = json!({
            "schema_version": CONFIG_SCHEMA,
            "storage": {"backend": "local", "local": {"path": storage}},
        });
        if declared {
            configuration["secrets"] = json!({"skarbiec": {"vault_file": vault}});
        }
        fs::write(
            home.join(".config/stado/config.json"),
            serde_json::to_vec_pretty(&configuration).unwrap(),
        )
        .expect("write the host's own Stado declaration");

        // The product reaches the host's Stado and Skarbiec at these two fixed
        // paths under the account's home. Both are the real binaries.
        let broker = real_skarbiec_binary();
        link(
            Path::new(env!("CARGO_BIN_EXE_stado")),
            &home.join(".stado/bin/stado"),
        );
        link(&broker, &home.join(".stado/bin/skarbiec"));

        let host = Self {
            root,
            home,
            storage,
            gnupg,
            vault,
            broker,
        };
        host.initialise_vault();
        host
    }

    /// Create the vault with real GnuPG keys, through the real broker.
    fn initialise_vault(&self) {
        let created = self
            .broker_command()
            .args(["init", OWNER])
            .output()
            .expect("the real Skarbiec broker runs");
        assert!(
            created.status.success(),
            "blocked: the real Skarbiec broker could not initialise an isolated vault\n{}",
            said(&created)
        );
        assert!(
            self.vault.is_file(),
            "the real broker reported success without writing {}",
            self.vault.display()
        );
    }

    /// The real broker, addressed at the isolated vault. Used only to seed and
    /// to read what the product wrote; never to answer for the product.
    pub fn broker_command(&self) -> Command {
        let mut command = Command::new(&self.broker);
        command
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", SYSTEM_PATH)
            .env("GNUPGHOME", &self.gnupg)
            .env("SKARBIEC_VAULT_FILE", &self.vault)
            .env(
                "SKARBIEC_AUDIT_FILE",
                self.home.join("skarbiec-audit.jsonl"),
            )
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    /// One `stado` invocation against the isolated host.
    ///
    /// `SKARBIEC_VAULT_FILE` is deliberately absent from this environment: the
    /// only thing that can point the command at the isolated vault is the
    /// declaration the host itself carries, which is what these cases are
    /// about.
    pub fn run(&self, arguments: &[&str], stdin: Option<&str>) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(arguments)
            .current_dir(self.root.path())
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", SYSTEM_PATH)
            .env("GNUPGHOME", &self.gnupg)
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &self.storage)
            .env("STADO_CONFIG", self.root.path().join("no-such-config.json"))
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the built stado binary runs");
        if let Some(payload) = stdin {
            child
                .stdin
                .take()
                .expect("the payload travels on stdin")
                .write_all(payload.as_bytes())
                .expect("write the payload");
        }
        child.wait_with_output().expect("stado finishes")
    }

    /// The persisted encrypted vault, parsed. This is the state every
    /// assertion about a write reads.
    pub fn vault_document(&self) -> Value {
        serde_json::from_slice(&self.vault_bytes()).expect("the persisted vault is JSON")
    }

    pub fn vault_bytes(&self) -> Vec<u8> {
        fs::read(&self.vault).expect("read the persisted vault")
    }

    pub fn vault_path(&self) -> &Path {
        &self.vault
    }
}

impl Drop for IsolatedHost {
    /// The isolated keyring's agent holds a socket inside the tempdir, so it
    /// is stopped before the directory goes away.
    fn drop(&mut self) {
        let _ = Command::new("gpgconf")
            .env("PATH", SYSTEM_PATH)
            .arg("--homedir")
            .arg(&self.gnupg)
            .args(["--kill", "gpg-agent"])
            .output();
    }
}

/// Both streams of one invocation, for a failure message.
pub fn said(output: &Output) -> String {
    format!(
        "exit: {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    )
}

pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The kernel's hostname, normalized the way the registry requires it.
fn hostname() -> String {
    let output = Command::new("hostname")
        .env_clear()
        .env("PATH", SYSTEM_PATH)
        .output()
        .expect("blocked: the real hostname executable could not start");
    assert!(
        output.status.success(),
        "blocked: the real hostname executable failed\n{}",
        said(&output)
    );
    let hostname = String::from_utf8(output.stdout)
        .expect("the kernel hostname is UTF-8")
        .trim()
        .to_lowercase();
    assert!(!hostname.is_empty(), "this host reports no hostname");
    hostname
}

fn release_platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("linux", "x86_64") => "linux-amd64",
        (os, arch) => {
            panic!("blocked: this area requires macOS arm64 or Linux amd64, got {os}-{arch}")
        }
    }
}

fn link(binary: &Path, at: &Path) {
    std::os::unix::fs::symlink(binary, at)
        .unwrap_or_else(|error| panic!("place {} at {}: {error}", binary.display(), at.display()));
}
