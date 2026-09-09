//! The isolated host every case here drives `stado disk-cleanup` against.
//!
//! One registry target, and it IS this machine: the target's `hostnames` carry
//! this kernel's own host name lower-cased, which is the identity
//! `resolve_canonical_policy` normalizes and matches, so the command resolves
//! its policy from this document and runs its local pass. `HOME`, the store
//! and the cleaner root are all inside one temporary directory and
//! `STADO_CONFIG` names a file that does not exist, so no case can reach the
//! operator's own registry, janitor state or caches.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};

/// The Cache Directory Tagging Standard signature `build_caches` requires
/// before it treats a directory as regenerable. Copied from the standard,
/// which is what the product's own reader compares against.
const CACHE_TAG: &str = "Signature: 8a477f597d28d172789f06886806bc55\n";

/// The janitor's own state file, relative to `HOME`.
const STATE_PATH: &str = ".cache/wisent-compute/disk-cleanup-state.json";

/// The registry target's name. Not a host name: the target is matched to this
/// machine by `hostnames`, and this is only what reports call it.
pub const TARGET: &str = "janitor-runner";

pub struct Journey {
    home: tempfile::TempDir,
    storage: PathBuf,
    cache_root: PathBuf,
}

impl Journey {
    /// An isolated host declaring the accepted policy below.
    pub fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/janitor-refusal-runs");
        std::fs::create_dir_all(&root).expect("the area's run root is ours to create");
        let home = tempfile::Builder::new()
            .prefix("refusal-")
            .tempdir_in(root)
            .expect("a temporary home");
        let storage = home.path().join("store");
        let cache_root = home.path().join("build-output");
        std::fs::create_dir_all(&storage).expect("the store root");
        std::fs::create_dir_all(&cache_root).expect("the cleaner root");
        let journey = Self {
            home,
            storage,
            cache_root,
        };
        journey.declare(journey.policy());
        journey
    }

    /// The declared policy this area starts from: enforcing, with watermarks
    /// above any real disk so a pass reaches its cleaners, and one cleaner
    /// rooted inside the fixture. Every number is a registry field bounded by
    /// `targets::validation_disk`; none of them tunes the product.
    pub fn policy(&self) -> Value {
        json!({
            "mode": "enforce",
            "check_interval_seconds": 60,
            "low_free_gb": 1_000_000,
            "target_free_gb": 1_000_001,
            "max_bytes_per_pass": 1_073_741_824_i64,
            "max_items_per_pass": 10,
            "max_scan_items": 1000,
            "max_pass_seconds": 30,
            "cleaners": {"build_caches": {"min_age_seconds": 86400, "root": self.cache_root}},
        })
    }

    /// Publish a canonical registry naming this machine and declaring `policy`.
    pub fn declare(&self, policy: Value) {
        let document = json!({
            "schema_version": 2,
            "coordinators": [],
            "targets": [{
                "name": TARGET,
                "kind": "local",
                "release_platform": release_platform(),
                "ssh": "nobody@127.0.0.1",
                "hostnames": [hostname()],
                "disk_cleanup": policy,
            }],
        });
        self.write_registry(
            &serde_json::to_string_pretty(&document).expect("the registry serializes"),
        );
    }

    /// Replace the canonical registry with exactly these bytes, which is how a
    /// document that does not parse gets in front of the command.
    pub fn write_registry(&self, document: &str) {
        std::fs::write(self.storage.join("registry.json"), document)
            .expect("the canonical registry is ours to write");
    }

    /// A real regenerable cache directory, backdated past the declared
    /// retention so an enforcing pass would delete it — the evidence a refused
    /// pass leaves it alone.
    pub fn eligible_cache(&self, name: &str) -> PathBuf {
        let directory = self.cache_root.join(name);
        std::fs::create_dir(&directory).expect("the cache directory");
        std::fs::write(directory.join("CACHEDIR.TAG"), CACHE_TAG).expect("the cache tag");
        std::fs::write(directory.join("payload.bin"), vec![0x5a; 8192]).expect("the cache payload");
        let touched = Command::new("/usr/bin/touch")
            .args(["-t", "202001010000"])
            .arg(&directory)
            .status()
            .expect("/usr/bin/touch ran");
        assert!(touched.success(), "the cache directory was not backdated");
        directory
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .env_clear()
            .env("HOME", self.home.path())
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("STADO_CONFIG", self.home.path().join("no-config.json"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &self.storage)
            .env("WC_PROVIDERS", "local");
        command
    }

    pub fn invoke(&self, args: &[&str]) -> Output {
        self.command().args(args).output().expect("stado ran")
    }

    /// Run one cleanup pass and return the disk report it printed.
    ///
    /// The command prints two documents, the disk pass and the memory pass, so
    /// the disk report is the first line rather than the whole stream.
    pub fn cleanup(&self, args: &[&str]) -> Value {
        let output = self.invoke(args);
        assert_eq!(
            output.status.code(),
            Some(0),
            "stado {} exited {:?}\nstdout:\n{}\nstderr:\n{}",
            args.join(" "),
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        let stdout = String::from_utf8(output.stdout).expect("the report is utf-8");
        let first = stdout.lines().next().expect("the pass printed a report");
        serde_json::from_str(first).expect("the disk report is one JSON document")
    }

    /// The janitor's persisted state, which is what `stado space report` reads
    /// back to an operator long after the command exited.
    pub fn persisted(&self) -> Value {
        let path = self.home.path().join(STATE_PATH);
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|error| panic!("no janitor state at {}: {error}", path.display()));
        serde_json::from_slice(&bytes).expect("the janitor state file is JSON")
    }

    /// The `janitor:` line `stado space report` prints for this host.
    pub fn reported_janitor_line(&self) -> String {
        let output = self.invoke(&["space", "report", TARGET]);
        let stdout = String::from_utf8(output.stdout).expect("the report is utf-8");
        stdout
            .lines()
            .find(|line| line.starts_with("janitor:"))
            .unwrap_or_else(|| panic!("space report printed no janitor line:\n{stdout}"))
            .to_string()
    }
}

/// This machine's own host name, lower-cased because the registry refuses a
/// name it would have to normalize.
fn hostname() -> String {
    let output = Command::new("/bin/hostname")
        .output()
        .expect("/bin/hostname ran");
    String::from_utf8_lossy(&output.stdout).trim().to_lowercase()
}

fn release_platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("linux", "x86_64") => "linux-amd64",
        (os, arch) => panic!("this area has no release platform for {os}-{arch}"),
    }
}
