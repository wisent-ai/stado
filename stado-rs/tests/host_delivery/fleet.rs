//! The isolated fleet these deliveries run against: this machine, in a
//! registry of its own.
//!
//! The one registry target carries this machine's kernel hostname, normalized
//! the way a registry write requires, so `target_is_this_host` answers yes and
//! the product's current-host branch does the transfer. `HOME` is inside the
//! fixture, so the managed run root the product writes into is the fixture's
//! own directory and nothing under the operator's home is touched.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::json;

/// The registry name of the isolated entry standing for this machine.
pub const TARGET: &str = "delivery-current-host";

/// The run this area delivers into. One canonical lowercase UUID, which is
/// the only shape the destination policy accepts below the managed run root.
pub const RUN: &str = "123e4567-e89b-12d3-a456-426614174000";

/// The managed area the product confines a delivery to, relative to the
/// target account's home.
const RUN_AREA: &str = ".stado/work/runs";

/// The registry document version this fixture writes.
const REGISTRY_SCHEMA: u32 = 2;

/// The tool directories the product's own `/bin/sh` guards and `rsync`
/// invocation are found in, fixed here so the delivery resolves the operating
/// system's real programs whatever the shell that started `cargo test`
/// exported. Nothing belonging to this fixture is on it.
const SYSTEM_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

pub struct Fleet {
    pub root: tempfile::TempDir,
    pub home: PathBuf,
    pub source: PathBuf,
    storage: PathBuf,
}

impl Fleet {
    pub fn new() -> Self {
        let root = tempfile::Builder::new()
            .prefix("host-delivery-")
            .tempdir()
            .expect("create the isolated delivery root");
        let home = root.path().join("home");
        let storage = root.path().join("storage");
        let source = root.path().join("source");
        for directory in [&home, &storage, &source] {
            fs::create_dir_all(directory).expect("create an isolated delivery directory");
        }
        fs::write(
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
        Self {
            root,
            home,
            source,
            storage,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", SYSTEM_PATH)
            .env("STADO_CONFIG", self.root.path().join("no-such-config.json"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &self.storage)
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    pub fn run(&self, arguments: &[&str]) -> Output {
        self.command()
            .args(arguments)
            .output()
            .expect("the built stado binary runs")
    }

    /// One delivery whose file list arrives on stdin, which is the shape that
    /// keeps selected names out of a shell's argv.
    pub fn deliver_selection(&self, destination: &str, file_list: &[u8]) -> Output {
        let mut child = self
            .command()
            .args([
                "host",
                "deliver",
                TARGET,
                self.source.to_str().unwrap(),
                destination,
                "--files-from",
                "-",
                "--json",
            ])
            .stdin(Stdio::piped())
            .spawn()
            .expect("the built stado binary starts");
        child
            .stdin
            .take()
            .expect("the file list travels on stdin")
            .write_all(file_list)
            .expect("write the file list");
        child.wait_with_output().expect("stado finishes")
    }

    /// Where a delivery named `name` lands: inside this fixture's own home.
    pub fn delivered(&self, name: &str) -> PathBuf {
        self.home.join(RUN_AREA).join(RUN).join(name)
    }

    /// The managed destination argument for `name`, relative to the account's
    /// home as the command requires.
    pub fn destination(name: &str) -> String {
        format!("{RUN_AREA}/{RUN}/{name}")
    }

    pub fn run_area(&self) -> PathBuf {
        self.home.join(RUN_AREA)
    }
}

pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Both streams, for a failure message.
pub fn said(output: &Output) -> String {
    format!("stdout={} stderr={}", stdout(output), stderr(output))
}

/// The permission bits the product left on a path it created or delivered.
pub fn delivered_mode(path: &Path) -> u32 {
    fs::metadata(path)
        .unwrap_or_else(|error| panic!("stat {}: {error}", path.display()))
        .permissions()
        .mode()
        & 0o777
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
