//! The isolated host, its registry, and the enforcing pass every case asks for.
//!
//! One registry target, and it IS this machine: its `hostnames` carry this
//! kernel's own host name lower-cased, so `stado agent --target` claims for
//! this host and `stado disk-cleanup` resolves the same policy. The low
//! watermark is below any real disk, because a host under disk pressure stops
//! claiming and every case here needs a claimed workload; the enforcing pass
//! is therefore asked for with `--to-target`, which is the declared way to run
//! one bounded enforcing pass on a host above its low watermark.
//!
//! The queue half of the journey — submit, agent, records — is [`crate::fleet`].

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output};

use serde_json::{json, Value};

/// The registry target's name; the machine is matched by `hostnames`.
pub const TARGET: &str = "janitor-runner";

/// The Cache Directory Tagging Standard signature `build_caches` requires
/// before it treats a directory as regenerable.
const CACHE_TAG: &str = "Signature: 8a477f597d28d172789f06886806bc55\n";

pub struct Journey {
    pub home: tempfile::TempDir,
    pub storage: PathBuf,
    cache_root: PathBuf,
    pub agent: Option<Child>,
}

impl Journey {
    pub fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/workload-hold-runs");
        std::fs::create_dir_all(&root).expect("the area's run root is ours to create");
        let home = tempfile::Builder::new()
            .prefix("hold-")
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
            agent: None,
        };
        journey.declare();
        journey
    }

    /// The canonical registry: this machine, one slot, and an enforcing policy
    /// whose low watermark leaves the host claiming while its target watermark
    /// is above the disk, so `--to-target` has something to reclaim toward.
    /// Every number is a registry field bounded by `targets::validation_disk`
    /// and none of them tunes the product.
    fn declare(&self) {
        let cleaner = json!({"min_age_seconds": 86400, "root": self.cache_root});
        let document = json!({
            "schema_version": 2,
            "coordinators": [],
            "targets": [{
                "name": TARGET,
                "kind": "local",
                "release_platform": release_platform(),
                "ssh": "nobody@127.0.0.1",
                "hostnames": [hostname()],
                "slots": 1,
                "max_concurrent": 1,
                "disk_cleanup": {
                    "mode": "enforce",
                    "check_interval_seconds": 60,
                    "low_free_gb": 1,
                    "target_free_gb": 1_000_000,
                    "max_bytes_per_pass": 1_073_741_824_i64,
                    "max_items_per_pass": 10,
                    "max_scan_items": 1000,
                    "max_pass_seconds": 30,
                    "cleaners": {"build_caches": cleaner},
                },
            }],
        });
        std::fs::write(
            self.storage.join("registry.json"),
            serde_json::to_string_pretty(&document).expect("the registry serializes"),
        )
        .expect("the canonical registry is ours to write");
    }

    pub fn home(&self) -> &Path {
        self.home.path()
    }

    /// A real regenerable cache directory, backdated past the declared
    /// retention: an enforcing pass deletes it, and a refused one cannot.
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

    pub fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .env_clear()
            .env("HOME", self.home.path())
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("STADO_CONFIG", self.home.path().join("no-config.json"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &self.storage)
            .env("WC_PROVIDERS", "local")
            .env("WC_LOCAL_SLOTS", "1")
            .env("WC_VAST_AUTO_LIST", "false");
        command
    }

    pub fn invoke(&self, args: &[&str]) -> Output {
        self.command().args(args).output().expect("stado ran")
    }

    /// Ask for one bounded enforcing pass and return the disk report. The
    /// command prints the disk pass and then the memory pass, so the disk
    /// report is the first line.
    pub fn reclaim(&self) -> Value {
        let args = ["disk-cleanup", "--once", "--to-target"];
        let output = self.invoke(&args);
        assert_eq!(
            output.status.code(),
            Some(0),
            "stado {} exited {:?}\nstderr:\n{}",
            args.join(" "),
            output.status.code(),
            String::from_utf8_lossy(&output.stderr),
        );
        let stdout = String::from_utf8(output.stdout).expect("the report is utf-8");
        serde_json::from_str(stdout.lines().next().expect("a report line"))
            .expect("the disk report is one JSON document")
    }

    /// The janitor's persisted state, which outlives every pass.
    pub fn persisted(&self) -> Value {
        let path = self
            .home
            .path()
            .join(".cache/wisent-compute/disk-cleanup-state.json");
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

impl Drop for Journey {
    fn drop(&mut self) {
        self.stop_agent();
        self.refuse_terminal_records(false);
    }
}

/// This machine's own host name, lower-cased because the registry refuses a
/// name it would have to normalize.
fn hostname() -> String {
    let output = Command::new("/bin/hostname")
        .output()
        .expect("/bin/hostname ran");
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .to_lowercase()
}

fn release_platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("linux", "x86_64") => "linux-amd64",
        (os, arch) => panic!("this area has no release platform for {os}-{arch}"),
    }
}
