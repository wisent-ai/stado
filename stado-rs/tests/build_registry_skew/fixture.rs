//! The seeded registry, the documents, and the product invocation these cases
//! share.
//!
//! Split out of `main.rs` so each file stays inside the three hundred line
//! limit this repository enforces on itself.

use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::Value;

/// The registry label for the machine running this test. A target's `name` is
/// a label in the document; the machine it means is the entry in `hostnames`,
/// which is why this test can declare its own kernel host name under a name
/// of its choosing and still be about this machine.
pub const LOCAL: &str = "skew-probe-host";

/// A second declared machine. No process here can ask what build is installed
/// there, which is the whole point of the unmeasured verdict below.
pub const REMOTE: &str = "skew-probe-other";

/// The two verdict slugs `registry doctor` prints, copied from its output.
pub const REFUSES: &str = "build-refuses-registry";
pub const UNREAD: &str = "unread-build-verdict";

/// A key inside a cleaner every build knows. No build in the fleet implements
/// it, and the schema says so by name — which is what makes the refusal
/// readable rather than a shrug.
pub const UNIMPLEMENTED_KEY: &str = "min_age_days";

/// A cleaner name this build does not know, spelled the way a newer document
/// would spell one. Since 2026-09-04 an unfamiliar cleaner *name* is skipped
/// and reported by the janitor rather than refusing the whole policy: refusing
/// it switched every cleaner off on the mini the moment `release_store` was
/// declared for a binary still queued to reach it.
pub const NEWER_CLEANER: &str = "cleaner_a_newer_build_knows";

/// This machine's own name, as the kernel answers it. A registry write is
/// refused outright when a declared host name is not normalized, which is why
/// it is lowercased here rather than passed through.
pub fn hostname() -> String {
    let out = Command::new("/bin/hostname")
        .output()
        .expect("the operating system reports its host name");
    String::from_utf8_lossy(&out.stdout).trim().to_lowercase()
}

/// A temp root carrying a local storage backend, and the product run against
/// it.
pub struct Harness {
    dir: tempfile::TempDir,
}

impl Harness {
    pub fn new() -> Self {
        let harness = Self {
            dir: tempfile::tempdir().expect("temp root"),
        };
        for sub in ["storage", "storage/host_health", "home"] {
            std::fs::create_dir_all(harness.root().join(sub)).expect("temp subdirectory");
        }
        harness
    }

    pub fn root(&self) -> PathBuf {
        self.dir.path().to_path_buf()
    }

    pub fn registry_path(&self) -> PathBuf {
        self.root().join("storage/registry.json")
    }

    /// Seed the canonical registry: this machine under the label `LOCAL`,
    /// carrying `cleaners`, and one further declared machine nothing here can
    /// ask.
    pub fn declare_registry(&self, cleaners: Value) {
        let document = serde_json::json!({
            "schema_version": 2,
            "coordinators": [],
            "targets": [
                {
                    "name": LOCAL,
                    "kind": "local",
                    "ssh": null,
                    "release_platform": "darwin-arm64",
                    "hostnames": [hostname()],
                    "disk_cleanup": {
                        "mode": "report",
                        "check_interval_seconds": 3600,
                        "low_free_gb": 100,
                        "target_free_gb": 200,
                        "max_bytes_per_pass": 68719476736_u64,
                        "max_items_per_pass": 512,
                        "max_scan_items": 4096,
                        "cleaners": cleaners,
                    },
                },
                {
                    "name": REMOTE,
                    "kind": "local",
                    "ssh": "someone@10.9.9.31",
                    "release_platform": "darwin-arm64",
                    "hostnames": [format!("{REMOTE}.local")],
                },
            ],
        });
        std::fs::write(
            self.registry_path(),
            serde_json::to_string_pretty(&document).expect("registry document"),
        )
        .expect("seed registry");
    }

    pub fn stado(&self, args: &[&str]) -> Output {
        let root = self.root();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_stado"));
        cmd.args(args)
            .env_clear()
            .env("HOME", root.join("home"))
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", root.join("storage"))
            // A set-but-missing STADO_CONFIG disables config-file discovery,
            // so the developer's own configuration cannot reach this run.
            .env("STADO_CONFIG", root.join("storage/no-such-config.json"));
        cmd.output().expect("stado binary runs")
    }

    /// `registry validate` against the seeded document.
    pub fn validate(&self) -> Output {
        let path = self.registry_path();
        self.stado(&["registry", "validate", path.to_str().expect("a utf-8 path")])
    }

    /// The version the running binary reports, which the refusal has to name:
    /// the finding is about the age of an installed build, so a sentence
    /// without a version cannot be acted on.
    pub fn installed_version(&self) -> String {
        let printed = stdout(&self.stado(&["--version"]));
        printed
            .split_whitespace()
            .find(|word| word.chars().next().is_some_and(char::is_numeric))
            .expect("--version prints a version")
            .to_string()
    }
}

pub fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

pub fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// The rows `registry doctor --json` printed for one finding slug.
pub fn findings(out: &Output, slug: &str) -> Vec<Value> {
    let report: Value =
        serde_json::from_str(&stdout(out)).expect("registry doctor --json prints one object");
    report
        .get("findings")
        .and_then(Value::as_array)
        .expect("a findings array")
        .iter()
        .filter(|finding| finding.get("finding").and_then(Value::as_str) == Some(slug))
        .cloned()
        .collect()
}

/// The detail sentence of one row.
pub fn detail(finding: &Value) -> String {
    finding
        .get("detail")
        .and_then(Value::as_str)
        .expect("a detail sentence")
        .to_string()
}

/// The cleaner set every build in the fleet implements.
pub fn accepted_cleaners() -> Value {
    serde_json::json!({ "build_caches": { "min_age_seconds": 86400 } })
}

/// The same cleaner carrying one key no build implements.
pub fn cleaner_with_an_unimplemented_key() -> Value {
    serde_json::json!({
        "build_caches": { "min_age_seconds": 86400, UNIMPLEMENTED_KEY: 1 },
    })
}

/// A cleaner name no build in the fleet knows yet.
pub fn cleaner_a_newer_build_knows() -> Value {
    serde_json::json!({ NEWER_CLEANER: { "min_age_seconds": 86400 } })
}
