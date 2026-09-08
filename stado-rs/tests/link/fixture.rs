//! The isolated registry, the product invocation, the beacon this area
//! publishes through the product, and the records it reads back off disk.
//!
//! Split out of `main.rs` so each file stays inside the three hundred line
//! limit this repository enforces on itself.

use std::path::PathBuf;
use std::process::{Command, Output};

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{json, Value};

use crate::machine;

/// The product's own default silence threshold
/// (`monitor::host_silence::DEFAULT_SILENCE_THRESHOLD_SECONDS`).
///
/// This constant is read, never exported: `STADO_SILENCE_THRESHOLD_SECONDS`
/// is removed from every child below, so no case here can pass because an
/// operator set a configuration value by hand, and the threshold the report
/// names is the product's own.
pub const THRESHOLD_SECONDS: i64 = 300;

/// The reader-refusal window the report publishes
/// (`cli::host::checks::REFUSAL_WINDOW_SECONDS`).
pub const REFUSAL_WINDOW_SECONDS: i64 = 3600;

/// The interface-change window one beacon reads, which is the beacon's own
/// default cadence (`deploy::host_link::DEFAULT_WINDOW_SECONDS`).
/// `WC_HEALTH_INTERVAL_SECONDS` is likewise removed from every child, so this
/// is the window the collector actually used.
pub const CHANGE_WINDOW_SECONDS: i64 = 300;

/// Slack allowed between an instant this test measured and an instant the
/// product stamped inside the same run: the probes are capped at five seconds
/// each and there are three of them, plus process start.
pub const SLACK_SECONDS: i64 = 120;

/// One storage root, one HOME, one registry, all inside a temporary
/// directory that dies with the case.
pub struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    /// An isolated registry whose single target IS this machine.
    ///
    /// The declared host name is the one the operating system reports here,
    /// lower cased, which is both the form a registry write demands and the
    /// form `deploy::host_channel::target_is_this_host` matches. No ssh
    /// destination is declared, so the product takes its current-host path
    /// and every read below runs this machine's own tools in this machine's
    /// own process — there is nothing on PATH for a fake to occupy.
    pub fn new() -> Self {
        let dir = tempfile::tempdir().expect("an isolated storage root");
        for sub in ["storage", "home"] {
            std::fs::create_dir_all(dir.path().join(sub)).expect("the isolated root is writable");
        }
        let registry = json!({
            "schema_version": 2,
            "targets": [{
                "name": machine::slug(),
                "kind": "local",
                "ssh": null,
                "release_platform": machine::release_platform(),
                "hostnames": [machine::hostname()],
                "slots": 1,
                "services": [],
            }],
            "coordinators": [],
        });
        std::fs::write(
            dir.path().join("storage/registry.json"),
            serde_json::to_vec_pretty(&registry).expect("the registry serialises"),
        )
        .expect("seed the isolated registry");
        Self { dir }
    }

    pub fn storage(&self) -> PathBuf {
        self.dir.path().join("storage")
    }

    /// The HOME every child is given: inside the temporary root, so a flow
    /// that expands `$HOME` reads this and never the operator's.
    pub fn home(&self) -> PathBuf {
        self.dir.path().join("home")
    }

    /// The registry name of the one target, which is this machine's own slug.
    pub fn host(&self) -> String {
        machine::slug()
    }

    pub fn stado(&self, args: &[&str]) -> Output {
        let storage = self.storage();
        Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(args)
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &storage)
            // A set-but-missing STADO_CONFIG disables config-file discovery,
            // so the operator's real configuration cannot reach a case.
            .env("STADO_CONFIG", storage.join("no-such-config.json"))
            .env("HOME", self.home())
            .env_remove("COMPUTE_API_KEY")
            .env_remove("COMPUTE_API_URL")
            .env_remove("WC_PROFILES_DIR")
            .env_remove("STADO_SILENCE_THRESHOLD_SECONDS")
            .env_remove("WC_HEALTH_INTERVAL_SECONDS")
            .env_remove("STADO_HOST_SSH_KEY_FILE")
            .output()
            .expect("the built stado binary runs")
    }

    /// Publish one beacon for this machine through the product, and return
    /// the document the product itself assembled.
    ///
    /// `--print` is the publish path without the control API: it validates
    /// the document, recognises the host as this machine, collects the `link`
    /// block right here — the real power log, the real unified log, the real
    /// tailnet tool if one exists — and prints exactly the bytes it would put
    /// on the wire. Nothing in this area writes a `link` block of its own,
    /// which is the whole difference from the fixture this replaced.
    pub fn publish_beacon(&self, reported_at: &str) -> Value {
        let source = self.dir.path().join("beacon.json");
        std::fs::write(
            &source,
            serde_json::to_vec(&json!({
                "host": machine::slug(),
                "reported_at": reported_at,
                "units": {},
            }))
            .expect("the beacon serialises"),
        )
        .expect("the isolated root is writable");
        let out = self.stado(&[
            "host",
            "publish-beacon",
            source.to_str().expect("a UTF-8 temporary path"),
            "--print",
        ]);
        assert_eq!(
            out.status.code(),
            Some(0),
            "publish-beacon refused this machine's own beacon: {}",
            stderr(&out)
        );
        serde_json::from_slice(&out.stdout).unwrap_or_else(|error| {
            panic!(
                "publish-beacon --print did not print one JSON document: {error}\n{}",
                stdout(&out)
            )
        })
    }

    /// Put a published document where the readers look for this host's
    /// beacon.
    pub fn seed_beacon(&self, document: &Value) {
        let path = self
            .storage()
            .join("host_health")
            .join(format!("{}.json", machine::slug()));
        std::fs::create_dir_all(path.parent().expect("a parent directory"))
            .expect("the storage root is writable");
        std::fs::write(
            path,
            serde_json::to_vec(document).expect("the beacon serialises"),
        )
        .expect("seed the beacon object");
    }

    /// Where this host's silence records live under the isolated root:
    /// `state/host_silence/<host>/`, which is the prefix
    /// `monitor::host_silence::SILENCE_PREFIX` gives them.
    pub fn silence_dir(&self) -> PathBuf {
        self.storage()
            .join("state/host_silence")
            .join(machine::slug())
    }

    /// Every silence record on disk for this host, oldest key first. The keys
    /// are compact UTC instants, so lexicographic order is chronological.
    pub fn silences(&self) -> Vec<Value> {
        let dir = self.silence_dir();
        let mut paths: Vec<PathBuf> = match std::fs::read_dir(&dir) {
            Err(_) => return Vec::new(),
            Ok(entries) => entries
                .map(|entry| entry.expect("a readable directory entry").path())
                .collect(),
        };
        paths.sort();
        paths
            .iter()
            .map(|path| {
                serde_json::from_str(
                    &std::fs::read_to_string(path).expect("a readable silence record"),
                )
                .expect("a silence record stays JSON")
            })
            .collect()
    }
}

pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The `--json` document, parsed.
pub fn document(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "host link --json did not print one JSON document: {error}\nstdout={}\nstderr={}",
            stdout(output),
            stderr(output)
        )
    })
}

/// One instant in the spelling the beacon writers emit: UTC, seconds, `Z`.
pub fn beacon_time(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// One instant out of a document, parsed, with the field named on failure.
pub fn instant(value: &Value, field: &str) -> DateTime<Utc> {
    let raw = value[field]
        .as_str()
        .unwrap_or_else(|| panic!("{field} is a string instant, got: {}", value[field]));
    DateTime::parse_from_rfc3339(raw)
        .unwrap_or_else(|error| panic!("{field} is not an RFC 3339 instant: {raw}: {error}"))
        .with_timezone(&Utc)
}

/// The blocker sentences the report carries, as they were written.
pub fn blockers(report: &Value) -> Vec<String> {
    report["blockers"]
        .as_array()
        .expect("blockers is an array")
        .iter()
        .map(|blocker| {
            blocker
                .as_str()
                .expect("a blocker is a sentence")
                .to_string()
        })
        .collect()
}
