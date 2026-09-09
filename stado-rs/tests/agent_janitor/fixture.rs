//! The isolated host, its registry, and the tree its janitor has to cross.
//!
//! One registry target, and it IS this machine: its `hostnames` carry this
//! kernel's own host name lower-cased, so `stado agent --target` resolves this
//! target, publishes capacity for it, and runs its declared janitor. `HOME`,
//! the store and the cleaner root are inside one temporary directory, so every
//! capacity document, janitor state file and job tree read here belongs to
//! this test.
//!
//! The agent and the documents it writes are [`crate::fleet`].

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output};

use serde_json::{json, Value};

/// The registry target's name; the machine is matched by `hostnames`.
pub const TARGET: &str = "janitor-runner";

/// The Cache Directory Tagging Standard signature `build_caches` reads before
/// it treats a directory as regenerable. Copied from the standard, which is
/// what the product's own reader compares against.
const CACHE_TAG: &str = "Signature: 8a477f597d28d172789f06886806bc55\n";

/// What a case wants from its host: a janitor with a long pass to spend, or a
/// host healthy enough to claim work.
pub enum Shape {
    /// Watermarks above any real disk, so every pass reaches its cleaners and
    /// walks the whole declared root.
    UnderPressure,
    /// A low watermark below any real disk, so the disk gate leaves the host
    /// accepting jobs.
    Claiming,
}

pub struct Journey {
    home: tempfile::TempDir,
    pub storage: PathBuf,
    cache_root: PathBuf,
    shards: Vec<PathBuf>,
    pub agent: Option<Child>,
}

impl Journey {
    pub fn new(shape: Shape) -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/agent-janitor-runs");
        std::fs::create_dir_all(&root).expect("the area's run root is ours to create");
        let home = tempfile::Builder::new()
            .prefix("agent-janitor-")
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
            shards: Vec::new(),
            agent: None,
        };
        journey.declare(shape);
        journey
    }

    /// The canonical registry naming this machine. Every number is a registry
    /// field bounded by `targets::validation_disk`: the watermarks decide only
    /// whether a pass reaches its cleaners, the scan ceiling is the
    /// validator's own maximum, and none of them tunes the product.
    fn declare(&self, shape: Shape) {
        let (low, target) = match shape {
            Shape::UnderPressure => (1_000_000, 1_000_001),
            Shape::Claiming => (1, 2),
        };
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
                    "low_free_gb": low,
                    "target_free_gb": target,
                    "max_bytes_per_pass": 1_073_741_824_i64,
                    "max_items_per_pass": 10,
                    "max_scan_items": 200_000,
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

    /// `count` real cache directories under the declared cleaner root,
    /// sharded so they can be built and torn down by several threads.
    ///
    /// Each carries the Cache Directory Tagging Standard signature the
    /// product's own reader looks for, so the walk opens and reads every one
    /// of them, and none is backdated, so all of them are inside the declared
    /// retention and the pass deletes nothing. That is the `healthy_noop`
    /// shape from the incident, and reading a tag is what makes the walk cost
    /// enough wall-clock to be worth measuring a publication cadence against.
    pub fn plant_tree(&mut self, count: usize) {
        const SHARDS: usize = 8;
        let per_shard = count / SHARDS;
        self.shards = (0..SHARDS)
            .map(|shard| self.cache_root.join(format!("shard-{shard}")))
            .collect();
        std::thread::scope(|scope| {
            for shard in &self.shards {
                scope.spawn(move || {
                    std::fs::create_dir(shard).expect("the shard root");
                    for index in 0..per_shard {
                        let directory = shard.join(format!("cache-{index}"));
                        std::fs::create_dir(&directory).expect("a cache directory");
                        std::fs::write(directory.join("CACHEDIR.TAG"), CACHE_TAG)
                            .expect("the cache tag the walk reads");
                    }
                });
            }
        });
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

    /// The capacity document this agent publishes for itself, if it has
    /// published one yet.
    pub fn capacity(&self) -> Option<Value> {
        let directory = self.storage.join("capacity");
        let entry = std::fs::read_dir(directory).ok()?.next()?.ok()?;
        serde_json::from_slice(&std::fs::read(entry.path()).ok()?).ok()
    }

    /// The janitor's persisted pass, once one has finished.
    pub fn persisted(&self) -> Option<Value> {
        let path = self
            .home
            .path()
            .join(".cache/wisent-compute/disk-cleanup-state.json");
        serde_json::from_slice(&std::fs::read(path).ok()?).ok()
    }

    /// The report of a COMPLETED pass written by the agent's own janitor.
    pub fn agent_pass(&self) -> Option<Value> {
        let state = self.persisted()?;
        let report = state.get("report")?.clone();
        (report["writer"] == "agent-tick" && report["duration_ms"].is_i64()).then_some(report)
    }
}

impl Drop for Journey {
    fn drop(&mut self) {
        self.stop_agent();
        // A planted tree is a hundred thousand directories; removing the
        // shards in parallel is what keeps the teardown near the build.
        let shards = std::mem::take(&mut self.shards);
        std::thread::scope(|scope| {
            for shard in &shards {
                scope.spawn(move || {
                    let _ = std::fs::remove_dir_all(shard);
                });
            }
        });
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
