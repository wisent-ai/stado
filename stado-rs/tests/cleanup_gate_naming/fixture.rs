//! The isolated registry, the janitor state file this machine's own disk
//! reader picks up, and the `stado host gates` invocation the cases share.
//!
//! Split out of `main.rs` so each file stays inside the three hundred line
//! limit this repository enforces on itself.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use chrono::{SecondsFormat, Utc};
use serde_json::Value;

/// The registry name of the host every case declares. It carries this
/// machine's own kernel host name, so `host gates` reads this machine's disk.
pub const TARGET: &str = "gate-naming-observation";

/// The janitor's own state file, relative to `HOME`, exactly as
/// `providers::local::disk_cleanup::state_relative_path` names it. The host
/// reader looks there and nowhere else.
pub const STATE_RELATIVE_PATH: &str = ".cache/wisent-compute/disk-cleanup-state.json";

/// The declared interval. `STALL_INTERVALS` is 4, so the stall window is
/// 1200s — the same 300s policy charless-mac-mini declares.
pub const INTERVAL_SECONDS: i64 = 300;

/// Comfortably outside that window.
pub const STALE_SECONDS: i64 = 4000;

/// The agent polls every ten seconds, so while a wedge lasts the newest
/// prevented pass is always seconds old.
pub const PREVENTED_SECONDS: i64 = 5;

/// The two gate words this area is about.
pub const STALLED: &str = "disk_cleanup_stalled";
pub const LOCK_HELD: &str = "disk_cleanup_lock_held";
pub const PRESSURE: &str = "disk_pressure_unresolved";

/// A watermark this machine measurably clears, and one it measurably cannot.
/// Both are declared in the registry the product reads; the free space they
/// are compared against is whatever `df` says on this disk right now.
#[derive(Clone, Copy)]
pub enum Headroom {
    Above,
    Below,
}

impl Headroom {
    fn watermarks(self) -> (i64, i64) {
        match self {
            Headroom::Above => (1, 2),
            Headroom::Below => (1_000_000, 1_000_001),
        }
    }
}

pub fn platform() -> &'static str {
    if std::env::consts::OS == "macos" {
        "darwin-arm64"
    } else {
        "linux-amd64"
    }
}

/// The kernel's own host name, lower-cased the way the registry validator
/// requires a declared name to be.
pub fn hostname() -> String {
    let output = Command::new("/bin/hostname")
        .output()
        .expect("the operating system reports its host name");
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .to_lowercase()
}

pub struct Fixture {
    root: tempfile::TempDir,
}

impl Fixture {
    /// An isolated registry declaring this machine with an armed janitor and
    /// the watermark this case needs, and a `HOME` inside the root so the
    /// state file read is this fixture's and not the operator's.
    pub fn declaring(headroom: Headroom) -> Self {
        let root = tempfile::tempdir().expect("an isolated storage root");
        let fixture = Self { root };
        std::fs::create_dir_all(fixture.state_path().parent().expect("a state directory"))
            .expect("the janitor state directory");
        // The host configuration reader executes the installed product path
        // its services use; without it the verdict is incomplete and decides
        // nothing.
        let bin = fixture.home().join(".stado/bin");
        std::fs::create_dir_all(&bin).expect("the isolated product installation");
        std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_stado"), bin.join("stado"))
            .expect("install the real product binary in the isolated host");
        let (low, target) = headroom.watermarks();
        let registry = serde_json::json!({
            "schema_version": 2,
            "coordinators": [],
            "targets": [{
                "name": TARGET,
                "kind": "local",
                "ssh": null,
                "release_platform": platform(),
                "hostnames": [hostname()],
                "services": [],
                "disk_cleanup": {
                    "mode": "report",
                    "check_interval_seconds": INTERVAL_SECONDS,
                    "low_free_gb": low,
                    "target_free_gb": target,
                    "max_bytes_per_pass": 68_719_476_736_i64,
                    "max_items_per_pass": 512,
                    "max_scan_items": 4096,
                    "cleaners": { "build_caches": { "min_age_seconds": 86400 } }
                }
            }],
        });
        std::fs::write(
            fixture.path().join("registry.json"),
            serde_json::to_vec_pretty(&registry).expect("registry serialises"),
        )
        .expect("seed the isolated registry");
        fixture
    }

    pub fn path(&self) -> &Path {
        self.root.path()
    }

    pub fn home(&self) -> PathBuf {
        self.root.path().to_path_buf()
    }

    pub fn state_path(&self) -> PathBuf {
        self.home().join(STATE_RELATIVE_PATH)
    }

    /// A janitor that completed a pass `success_seconds_ago`, and — when the
    /// host recorded one — was last turned away from the run lock
    /// `prevented_seconds_ago`. The document is the shape
    /// `disk_cleanup`'s `write_state` writes and `host_disk::parse_state`
    /// reads: an RFC 3339 success inside `report`, an epoch-second
    /// `last_prevented_at` beside it.
    pub fn record_janitor(&self, success_seconds_ago: i64, prevented_seconds_ago: Option<i64>) {
        let now = Utc::now();
        let stamp = now - chrono::Duration::seconds(success_seconds_ago);
        let mut document = serde_json::json!({
            "report": {
                "last_success_at": stamp.to_rfc3339_opts(SecondsFormat::Secs, true),
                "outcome": if prevented_seconds_ago.is_some() { "lock_busy" } else { "ok" },
            },
            "last_attempt_at": now.timestamp(),
        });
        if let Some(prevented) = prevented_seconds_ago {
            document["last_prevented_at"] = serde_json::json!(now.timestamp() - prevented);
        }
        std::fs::write(
            self.state_path(),
            serde_json::to_vec(&document).expect("the janitor state serialises"),
        )
        .expect("write the janitor state");
    }

    /// A janitor being refused the run lock on every tick with nothing having
    /// got through for the whole stall window — the wedge.
    pub fn record_wedged_janitor(&self) {
        self.record_janitor(STALE_SECONDS, Some(PREVENTED_SECONDS));
    }

    /// A janitor that is simply silent: stale success, nothing recording that
    /// anything was turned away.
    pub fn record_silent_janitor(&self) {
        self.record_janitor(STALE_SECONDS, None);
    }

    /// A janitor that completed a pass well inside its own interval.
    pub fn record_healthy_janitor(&self) {
        self.record_janitor(INTERVAL_SECONDS / 10, None);
    }

    fn stado(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(args)
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.path())
            .env("STADO_CONFIG", self.path().join("no-such-config.json"))
            .env("HOME", self.home())
            .env_remove("COMPUTE_API_KEY")
            .env_remove("COMPUTE_API_URL")
            .env_remove("STADO_API_URL")
            .env_remove("WC_PROFILES_DIR")
            .output()
            .expect("the built stado binary runs")
    }

    /// `stado host gates TARGET --json` — the operator console's payload.
    pub fn gates_report(&self) -> (Value, Output) {
        let output = self.stado(&["host", "gates", TARGET, "--json"]);
        let report = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "stdout was not one JSON report: {error}\nstdout={}\nstderr={}",
                stdout(&output),
                stderr(&output)
            )
        });
        (report, output)
    }

    /// The same command without `--json`: the lines an operator reads.
    pub fn gates_lines(&self) -> Output {
        self.stado(&["host", "gates", TARGET])
    }
}

pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The words the report lists under `key`, which is `blockers` or `notes`.
pub fn words(report: &Value, key: &str) -> Vec<String> {
    report[key]
        .as_array()
        .unwrap_or_else(|| panic!("the report carries {key}: {report:#}"))
        .iter()
        .map(|word| {
            word.as_str()
                .expect("every entry is a condition name")
                .to_string()
        })
        .collect()
}

/// The sentence `host gates` fails with, which names every blocker.
pub fn refusal(output: &Output) -> String {
    stderr(output)
        .lines()
        .find_map(|line| line.strip_prefix("Error: "))
        .unwrap_or_else(|| panic!("the command printed no refusal: {}", stderr(output)))
        .to_string()
}
