//! The isolated current-host fixture every case in this area drives.
//!
//! One local registry whose single target names the machine executing the
//! test, so `deploy::host_channel::target_is_this_host` is true and the
//! product takes its current-host path: `stado disk-cleanup` walks a real
//! directory tree with real directory descriptors, `/bin/df` and
//! `/usr/bin/tmutil` are the ones on this machine, and the pass writes its
//! state document where an operator would look for it. No executable is
//! substituted and the fixture declares no SSH destination, so a regression
//! that stopped resolving this host would fail rather than reach elsewhere.
//!
//! Isolation is the fixture's other job: a fresh tempdir per case, with
//! `HOME`, `TMPDIR` and the local storage root inside it and `STADO_CONFIG`
//! naming a path that does not exist, so the operator's own registry,
//! configuration and vault are unreachable. The one declared cleaner is
//! rooted inside that tempdir, so every path this area's passes may remove is
//! a path the case created itself.

use std::fs::{self, File, FileTimes};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, SystemTime};

use serde_json::Value;

use crate::native::{hostname, said, SYSTEM_PATH};

/// The registry target name this area declares for the current machine.
pub const TARGET: &str = "disk-scope-current-host";

/// The `CACHEDIR.TAG` first line the product requires before it treats a
/// directory as a regenerable build cache
/// (`deploy::host_build_caches::CACHEDIR_SIGNATURE`).
pub const CACHEDIR_TAG: &str = "Signature: 8a477f597d28d172789f06886806bc55\n";

/// The janitor's own state document, relative to the target account's home
/// (`providers::local::disk_cleanup::state_relative_path`).
pub const JANITOR_STATE: &str = ".cache/wisent-compute/disk-cleanup-state.json";

/// The writer name the disk pass stamps its report with, which is how the
/// disk document is told apart from the memory document `disk-cleanup` prints
/// beside it.
const DISK_WRITER: &str = "disk-cleanup-cli";

/// Registry-schema configuration for the fixture policy. `schema_version` is
/// the canonical registry version this build reads and the rest are the
/// janitor's declared tuning knobs; the two watermarks are deliberately far
/// above any real machine's free space so the pressure gate is active and an
/// enforcing pass reaches the one declared cleaner instead of reporting a
/// healthy no-op. Copied from a live `stado space report` on this host.
const SCHEMA_VERSION: u32 = 2;
const CHECK_INTERVAL_SECONDS: u32 = 3_600;
const LOW_FREE_GB: u64 = 3_999_999;
const TARGET_FREE_GB: u64 = 4_000_000;
const MAX_SCAN_ITEMS: u32 = 4_096;
/// One day, the age gate the declared cleaner is given: a build cache younger
/// than this is `too_young` and is measured but never removed.
pub const MIN_AGE_SECONDS: u32 = 86_400;
/// The default per-pass budgets: large enough that no case hits them unless it
/// declares its own.
pub const WHOLE_GIB: u64 = 1_073_741_824;
pub const MANY_ITEMS: u32 = 32;

/// Old enough for the age gate above, by a wide margin.
const AGED_DAYS: u64 = 3;

/// The isolated host: its tempdir, the registry naming this machine, and the
/// build-cache scope that registry declares.
pub struct Host {
    _dir: tempfile::TempDir,
    pub root: PathBuf,
    pub home: PathBuf,
    pub storage: PathBuf,
    /// The one directory the declared cleaner may reclaim from.
    pub cache_root: PathBuf,
    pub hostname: String,
}

impl Host {
    /// A fixture declaring the enforcing policy, whose one cleaner is rooted
    /// at a build-cache directory inside this tempdir.
    pub fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("stado-disk-scope-")
            .tempdir()
            .expect("create the isolated disk-scope host");
        let root = dir.path().to_path_buf();
        let home = root.join("home");
        let storage = root.join("storage");
        let cache_root = home.join("build-cache");
        for directory in [&home, &storage, &cache_root, &root.join("tmp")] {
            fs::create_dir_all(directory).expect("create isolated fixture directory");
        }
        // The host configuration reader executes the same installed product
        // path its services use, not a replacement command or a PATH lookup.
        let bin = home.join(".stado/bin");
        fs::create_dir_all(&bin).expect("create isolated product installation");
        std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_stado"), bin.join("stado"))
            .expect("install the real product binary in the isolated host");
        let host = Self {
            _dir: dir,
            root,
            home,
            storage,
            cache_root,
            hostname: hostname(),
        };
        host.declare(WHOLE_GIB, MANY_ITEMS);
        host
    }

    /// Write the registry with the enforcing policy under the given per-pass
    /// budgets, which is how the budget cases declare a bound a pass will
    /// actually reach.
    pub fn declare(&self, max_bytes_per_pass: u64, max_items_per_pass: u32) {
        let document = format!(
            r#"{{
  "schema_version": {SCHEMA_VERSION},
  "targets": [
    {{
      "name": "{TARGET}",
      "kind": "local",
      "ssh": null,
      "release_platform": "{platform}",
      "hostnames": [{hostname:?}],
      "services": [],
      "disk_cleanup": {{
        "mode": "enforce",
        "check_interval_seconds": {CHECK_INTERVAL_SECONDS},
        "low_free_gb": {LOW_FREE_GB},
        "target_free_gb": {TARGET_FREE_GB},
        "max_bytes_per_pass": {max_bytes_per_pass},
        "max_items_per_pass": {max_items_per_pass},
        "max_scan_items": {MAX_SCAN_ITEMS},
        "cleaners": {{
          "build_caches": {{
            "min_age_seconds": {MIN_AGE_SECONDS},
            "root": {root:?}
          }}
        }}
      }}
    }}
  ],
  "coordinators": []
}}
"#,
            platform = release_platform(),
            hostname = self.hostname,
            root = self.cache_root.to_string_lossy(),
        );
        fs::write(self.storage.join("registry.json"), document).expect("write fixture registry");
    }

    /// Run the built binary with nothing of the operator's environment left.
    pub fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(args)
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", SYSTEM_PATH)
            .env("TMPDIR", self.root.join("tmp"))
            .env("STADO_CONFIG", self.root.join("no-such-config.json"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &self.storage)
            .env("WC_PROVIDERS", "local")
            .env("NO_COLOR", "1")
            .output()
            .expect("the built stado binary did not start")
    }

    /// One JSON document from a command that had to succeed.
    pub fn json(&self, args: &[&str]) -> Value {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "{args:?} exited {:?}\nstderr:\n{}",
            output.status.code(),
            said(&output.stderr),
        );
        serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "{args:?} did not print one JSON document: {error}\n{}",
                said(&output.stdout)
            )
        })
    }

    /// Run one cleanup pass and return the disk report it printed.
    ///
    /// `disk-cleanup` prints the disk pass and the memory pass as one JSON
    /// document each, so the disk one is selected by its own writer name
    /// rather than by position.
    pub fn cleanup_pass(&self, args: &[&str]) -> Value {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "{args:?} exited {:?}\nstderr:\n{}",
            output.status.code(),
            said(&output.stderr),
        );
        let printed = said(&output.stdout);
        for line in printed.lines().filter(|line| !line.trim().is_empty()) {
            let document: Value = serde_json::from_str(line).unwrap_or_else(|error| {
                panic!("{args:?} printed a non-JSON line: {error}\n{line}")
            });
            if document["writer"] == DISK_WRITER {
                return document;
            }
        }
        panic!("{args:?} printed no {DISK_WRITER} report:\n{printed}");
    }

    /// The janitor's state document, or `None` when no pass has written one.
    pub fn janitor_state(&self) -> Option<Value> {
        let raw = fs::read_to_string(self.under_home(JANITOR_STATE)).ok()?;
        Some(serde_json::from_str(&raw).expect("the janitor state document is JSON"))
    }

    /// The path under this fixture's home that a cleaner or reader touches.
    pub fn under_home(&self, relative: &str) -> PathBuf {
        self.home.join(relative)
    }

    /// A directory of `payload_mib` under `parent`. `tagged` writes the cache
    /// tag that makes it a candidate at all; `aged` moves its mtime past the
    /// declared age gate, which is the timestamp `Walk::old_enough` reads.
    pub fn seed_tree(
        &self,
        parent: &Path,
        name: &str,
        payload_mib: usize,
        tagged: bool,
        aged: bool,
    ) -> PathBuf {
        let tree = parent.join(name);
        fs::create_dir_all(&tree).expect("create the scope this case owns");
        if tagged {
            fs::write(tree.join(CACHE_TAG_NAME), CACHEDIR_TAG).expect("write the cache tag");
        }
        let mut file = File::create(tree.join(PAYLOAD_NAME)).expect("create the scope's payload");
        let block = vec![0u8; 1 << 20];
        for _ in 0..payload_mib {
            file.write_all(&block).expect("write the scope's payload");
        }
        file.sync_all().expect("flush the scope's payload");
        drop(file);
        if aged {
            let old = SystemTime::now() - Duration::from_secs(AGED_DAYS * 24 * 60 * 60);
            File::open(&tree)
                .expect("open the scope to age it")
                .set_times(FileTimes::new().set_accessed(old).set_modified(old))
                .expect("age the scope past the declared age gate");
        }
        tree
    }
}

/// The two names every seeded tree carries: the tag the product reads as
/// permission, and the payload whose blocks are the bytes under test.
pub const CACHE_TAG_NAME: &str = "CACHEDIR.TAG";
pub const PAYLOAD_NAME: &str = "payload.bin";

/// The build cache cleaner's own row in a pass report or state document.
pub fn build_caches(report: &Value) -> &Value {
    &report["cleaners"]["build_caches"]
}

/// Whether the payload this fixture wrote is still on disk.
pub fn payload_kept(tree: &Path) -> bool {
    tree.join(PAYLOAD_NAME).is_file()
}

fn release_platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("macos", _) => "darwin-amd64",
        (_, "aarch64") => "linux-arm64",
        _ => "linux-amd64",
    }
}
