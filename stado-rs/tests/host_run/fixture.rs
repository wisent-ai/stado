//! The isolated machine these run journeys drive: this one.
//!
//! The only registry target carries this machine's kernel hostname,
//! normalized the way a registry write requires, so the product takes its
//! current-host branch and really executes processes here. Nothing in the
//! fixture names a remote destination and nothing is substituted on `PATH`
//! except the fixture's own `.cargo/bin`, which holds symlinks to the
//! operating system's real Cargo and rustc.
//!
//! `HOME` is inside a fresh tempdir, so the managed run area the product
//! writes, executes in and removes is the fixture's own directory tree.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{json, Value};

/// The registry name of the isolated entry standing for this machine.
pub const TARGET: &str = "run-current-host";

/// The managed area every run path must sit inside.
pub const RUN_AREA: &str = ".stado/work/runs";

/// The registry document version this fixture writes.
const REGISTRY_SCHEMA: u32 = 2;

/// Owner-only mode for a program this fixture writes into a run directory.
const OWNER_ONLY_EXECUTABLE: u32 = 0o700;

/// The operating system's own tool directories. The product's scripts export
/// their own `PATH` on top of this; nothing of the fixture's is on it.
const SYSTEM_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

pub struct Fixture {
    root: tempfile::TempDir,
    home: PathBuf,
    rustup_home: Option<PathBuf>,
}

impl Fixture {
    pub fn new() -> Self {
        let root = tempfile::Builder::new()
            .prefix("host-run-")
            .tempdir()
            .expect("create the isolated run root");
        let home = root.path().join("home");
        let storage = root.path().join("storage");
        std::fs::create_dir_all(home.join(RUN_AREA)).expect("create the managed run area");
        std::fs::create_dir_all(&storage).expect("create the isolated storage root");
        std::fs::write(
            storage.join("registry.json"),
            serde_json::to_vec_pretty(&json!({
                "schema_version": REGISTRY_SCHEMA,
                "targets": [{
                    "name": TARGET,
                    "kind": "local",
                    "release_platform": release_platform(),
                    "hostnames": [hostname()],
                    "services": [],
                }],
                "coordinators": [],
            }))
            .unwrap(),
        )
        .expect("write the isolated registry");

        let (cargo, rustc, rustup_home) = installed_toolchain();
        let cargo_bin = home.join(".cargo/bin");
        std::fs::create_dir_all(&cargo_bin).expect("create the account's Cargo directory");
        std::os::unix::fs::symlink(cargo, cargo_bin.join("cargo")).expect("link the real Cargo");
        std::os::unix::fs::symlink(rustc, cargo_bin.join("rustc")).expect("link the real rustc");

        Self {
            root,
            home,
            rustup_home,
        }
    }

    /// Create one managed run directory and return it.
    pub fn run(&self, name: &str) -> PathBuf {
        let path = self.home.join(RUN_AREA).join(name);
        std::fs::create_dir_all(&path).expect("create a managed run directory");
        path
    }

    /// The shared run root, which names no run of its own.
    pub fn run_root(&self) -> PathBuf {
        self.home.join(RUN_AREA)
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    pub fn command(&self, arguments: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .args(arguments)
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", SYSTEM_PATH)
            .env("STADO_CONFIG", self.root.path().join("no-such-config.json"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.root.path().join("storage"))
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(rustup_home) = &self.rustup_home {
            command.env("RUSTUP_HOME", rustup_home);
        }
        command
    }

    pub fn run_stado(&self, arguments: &[&str]) -> Output {
        self.command(arguments)
            .output()
            .expect("the built stado binary runs")
    }
}

/// Write one executable program into a managed run directory.
pub fn executable(path: &Path, body: &str) {
    std::fs::write(path, body).expect("write the program");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(OWNER_ONLY_EXECUTABLE))
        .expect("make the program executable");
}

pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

pub fn said(output: &Output) -> String {
    format!("stdout={} stderr={}", stdout(output), stderr(output))
}

pub fn receipt(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("invalid receipt ({error}): {}", said(output)))
}

/// The real Cargo, rustc and rustup home this machine has installed.
///
/// Resolved from the environment `cargo test` itself runs under, so the
/// fixture links the same toolchain the operator uses and never a stand-in.
fn installed_toolchain() -> (PathBuf, PathBuf, Option<PathBuf>) {
    let operator_home = std::env::var_os("HOME").map(PathBuf::from);
    let rustup_home = std::env::var_os("RUSTUP_HOME")
        .map(PathBuf::from)
        .or_else(|| operator_home.as_ref().map(|home| home.join(".rustup")));
    let cargo = std::env::var_os("CARGO")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute() && path.is_file())
        .or_else(|| {
            operator_home
                .as_ref()
                .map(|home| home.join(".cargo/bin/cargo"))
                .filter(|path| path.is_file())
        })
        .expect("blocked: this area needs the installed Cargo it is running under");
    let rustc = cargo
        .parent()
        .map(|directory| directory.join("rustc"))
        .filter(|path| path.is_file())
        .expect("blocked: Cargo's installed directory carries no rustc");
    (cargo, rustc, rustup_home)
}

/// The kernel's hostname, normalized the way a registry write requires: the
/// leading label, lowercased.
fn hostname() -> String {
    let reported = Command::new("hostname")
        .env("PATH", SYSTEM_PATH)
        .output()
        .expect("this machine answers hostname");
    let name = String::from_utf8(reported.stdout).expect("the hostname is UTF-8");
    let label = name
        .trim()
        .split('.')
        .next()
        .unwrap_or_default()
        .to_string();
    assert!(!label.is_empty(), "this machine reports no hostname label");
    label.to_ascii_lowercase()
}

fn release_platform() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "darwin-arm64"
    } else {
        "darwin-amd64"
    }
}
