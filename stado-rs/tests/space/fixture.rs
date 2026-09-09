//! The isolated current-host fixture every case in this area drives.
//!
//! Shaped after `tests/host_exec/main.rs`: an isolated local registry whose
//! one target names the machine executing the test, so
//! `host_channel::target_is_this_host` is true and the production code takes
//! its current-host path — `/bin/df`, `/usr/bin/du`, `/usr/bin/tmutil` and
//! this binary's own janitor run for real. No executable is substituted and
//! the fixture declares no SSH destination, so a regression that stopped
//! resolving this host would fail rather than quietly reach somewhere else.
//!
//! Isolation is the fixture's other job: a fresh tempdir per case, `HOME`,
//! `TMPDIR` and the local storage root inside it, and `STADO_CONFIG` pointing
//! at a path that does not exist, so the operator's own registry, vault and
//! configuration are unreachable. Every stage this area applies is selected by
//! name and sweeps only roots below that tempdir.

use std::fs::{self, File, FileTimes};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, SystemTime};

use serde_json::Value;

use crate::system::{hostname, said, SYSTEM_PATH};

/// The registry target name this area declares for the current machine.
pub const TARGET: &str = "space-current-host";
/// A target that names this machine and declares no cleanup scope at all.
pub const UNDECLARED_TARGET: &str = "space-undeclared-scope";

/// The `CACHEDIR.TAG` first line the product requires before it will treat a
/// directory as a regenerable build cache
/// (`deploy::host_build_caches::CACHEDIR_SIGNATURE`).
pub const CACHEDIR_TAG: &str = "Signature: 8a477f597d28d172789f06886806bc55\n";

/// The release build scratch root the `build_scratch` stage sweeps, relative
/// to the target account's home (`deploy::host_reclaim::BUILD_WORK_ROOT`).
pub const BUILD_WORK_ROOT: &str = ".stado/build-work";
/// Where an applied reclamation records itself, relative to that same home
/// (`deploy::host_reclaim::AUDIT_LOG`).
pub const AUDIT_LOG: &str = ".stado/audit/host-reclaim.jsonl";
/// The janitor's own state document, relative to that home
/// (`providers::local::disk_cleanup::state_relative_path`).
pub const JANITOR_STATE: &str = ".cache/wisent-compute/disk-cleanup-state.json";

/// Registry-schema configuration for the fixture policy. `schema_version` is
/// the canonical registry version this build reads; the rest are the janitor's
/// tuning knobs, and the two watermarks are deliberately far above any real
/// machine's free space so the pressure gate is active and the `enforce` pass
/// reaches the one cleaner declared below instead of reporting `healthy_noop`.
const SCHEMA_VERSION: u32 = 2;
const CHECK_INTERVAL_SECONDS: u32 = 3_600;
const LOW_FREE_GB: u64 = 3_999_999;
const TARGET_FREE_GB: u64 = 4_000_000;
const MAX_BYTES_PER_PASS: u64 = 1_073_741_824;
const MAX_ITEMS_PER_PASS: u32 = 32;
const MAX_SCAN_ITEMS: u32 = 4_096;
const MIN_AGE_SECONDS: u32 = 86_400;

/// Old enough for every age gate in the capability: the reclamation stages
/// refuse anything younger than one day and the cleaner declares the same.
const AGED_DAYS: u64 = 3;

/// The isolated host: its tempdir, the registry naming this machine, and the
/// build-cache scope that registry declares.
pub struct Host {
    _dir: tempfile::TempDir,
    pub root: PathBuf,
    pub home: PathBuf,
    pub storage: PathBuf,
    pub cache_root: PathBuf,
    pub hostname: String,
}

impl Host {
    /// A fixture declaring the enforcing cleanup policy, whose one cleaner is
    /// rooted at a build-cache directory inside this tempdir.
    pub fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("stado-space-")
            .tempdir()
            .expect("create the isolated space journey");
        let root = dir.path().to_path_buf();
        let home = root.join("home");
        let storage = root.join("storage");
        let cache_root = home.join("build-cache");
        for directory in [&home, &storage, &cache_root, &root.join("tmp")] {
            fs::create_dir_all(directory).expect("create isolated journey directory");
        }
        let host = Self {
            _dir: dir,
            root,
            home,
            storage,
            cache_root,
            hostname: hostname(),
        };
        host.declare_policy();
        host
    }

    /// The registry document with the enforcing policy and its one cleaner.
    pub fn declare_policy(&self) {
        self.declare(&self.policy());
    }

    /// The enforcing policy this area declares: the tuning constants above and
    /// one cleaner, `build_caches`, rooted inside this fixture's tempdir.
    ///
    /// One builder, because a case that restated these numbers would be a
    /// second source for the watermarks the whole area is measured against.
    pub fn policy(&self) -> String {
        format!(
            r#"{{
        "mode": "enforce",
        "check_interval_seconds": {CHECK_INTERVAL_SECONDS},
        "low_free_gb": {LOW_FREE_GB},
        "target_free_gb": {TARGET_FREE_GB},
        "max_bytes_per_pass": {MAX_BYTES_PER_PASS},
        "max_items_per_pass": {MAX_ITEMS_PER_PASS},
        "max_scan_items": {MAX_SCAN_ITEMS},
        "cleaners": {{"build_caches": {{"min_age_seconds": {MIN_AGE_SECONDS}, "root": {root:?}}}}}
      }}"#,
            root = self.cache_root.to_string_lossy(),
        )
    }

    /// The same registry with `disk_cleanup` replaced by `declaration`, which
    /// is a JSON object or the literal `null`.
    pub fn declare(&self, declaration: &str) {
        self.write_registry(TARGET, &format!(r#","disk_cleanup": {declaration}"#));
    }

    /// The same registry with `disk_cleanup` replaced by `declaration` and the
    /// target declaring the `stado` version installed on it.
    ///
    /// The version is part of the cleaner contract, not decoration: a cleaner
    /// name is accepted for a host only when the binary running there can
    /// parse it, so a fixture that declares no version can arm nothing.
    pub fn declare_running(&self, declaration: &str, stado_version: &str) {
        self.write_registry(
            TARGET,
            &format!(
                r#","managed_versions": {{"stado": "{stado_version}"}},"disk_cleanup": {declaration}"#
            ),
        );
    }

    /// A registry whose only target names this machine and declares nothing
    /// about cleanup, so no scope for the capability exists.
    pub fn declare_no_scope(&self) {
        self.write_registry(UNDECLARED_TARGET, "");
    }

    fn write_registry(&self, name: &str, cleanup: &str) {
        let document = format!(
            r#"{{
  "schema_version": {SCHEMA_VERSION},
  "targets": [
    {{
      "name": "{name}",
      "kind": "local",
      "ssh": null,
      "release_platform": "{platform}",
      "hostnames": [{hostname:?}],
      "services": []{cleanup}
    }}
  ],
  "coordinators": []
}}
"#,
            platform = release_platform(),
            hostname = self.hostname,
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

    /// The path under this fixture's home that a stage or cleaner sweeps.
    pub fn under_home(&self, relative: &str) -> PathBuf {
        self.home.join(relative)
    }

    /// A directory of `payload_mib` under `parent`, aged past every gate. The
    /// tag is what makes a directory a regenerable build cache to the product.
    pub fn seed_tree(
        &self,
        parent: &Path,
        name: &str,
        payload_mib: usize,
        tagged: bool,
    ) -> PathBuf {
        let tree = parent.join(name);
        fs::create_dir_all(&tree).expect("create the fixture's own scope");
        if tagged {
            fs::write(tree.join("CACHEDIR.TAG"), CACHEDIR_TAG).expect("write the cache tag");
        }
        let mut file = File::create(tree.join("payload.bin")).expect("create the scope's payload");
        let block = vec![0u8; 1 << 20];
        for _ in 0..payload_mib {
            file.write_all(&block).expect("write the scope's payload");
        }
        file.sync_all().expect("flush the scope's payload");
        drop(file);
        let aged = SystemTime::now() - Duration::from_secs(AGED_DAYS * 24 * 60 * 60);
        File::open(&tree)
            .expect("open the scope to age it")
            .set_times(FileTimes::new().set_accessed(aged).set_modified(aged))
            .expect("age the scope past the reclamation gates");
        tree
    }
}

/// Every path a reclamation reported, so a case can prove that nothing
/// outside its own tempdir was ever named.
pub fn reported_paths(stage: &Value) -> Vec<String> {
    stage["paths"]
        .as_array()
        .expect("a stage reports its paths")
        .iter()
        .map(|path| path.as_str().expect("a path is a string").to_string())
        .collect()
}

/// The one stage a reclamation ran.
pub fn only_stage<'a>(report: &'a Value, expected: &str) -> &'a Value {
    let stages = report["stages"]
        .as_array()
        .expect("the reclamation reports its stages");
    assert_eq!(stages.len(), 1, "expected one stage, got {stages:?}");
    assert_eq!(stages[0]["stage"], expected);
    &stages[0]
}

fn release_platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("macos", _) => "darwin-amd64",
        (_, "aarch64") => "linux-arm64",
        _ => "linux-amd64",
    }
}
