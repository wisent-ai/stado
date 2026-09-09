//! The tempdir fleet every case in this area drives the built binary against.
//!
//! One machine is involved and it is this one: the registry target's
//! `hostnames` carry this host's own kernel name from `/bin/hostname`, so
//! `stado host link` resolves it as a local target and runs its channel
//! locally. The store is a `tempfile::TempDir` behind the product's own
//! `WC_STORAGE_BACKEND=local`, `HOME` is a second tempdir, and `STADO_CONFIG`
//! points at a file that does not exist — so the operator's registry, beacons,
//! resolver sockets and services are never read and never touched, and every
//! assertion reads a blob this run made on a real disk.

use std::path::Path;
use std::process::{Command, Output};

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{json, Value};

/// The registry name this machine is declared under.
pub const TARGET: &str = "here";

/// The registry name of the authority in [`Fleet::with_unreachable_authority`].
pub const AUTHORITY: &str = "silence-test-authority";

/// The store prefix silence records live under, as the product declares it.
pub const SILENCE_PREFIX: &str = "state/host_silence";

/// The store prefix reader refusals live under, as the product declares it.
pub const REFUSAL_PREFIX: &str = "state/reader_refusals";

/// This machine's kernel host name, normalized the way the registry validator
/// demands ("must be normalized as '<lowercase>'").
pub fn kernel_hostname() -> String {
    let out = Command::new("/bin/hostname")
        .output()
        .expect("hostname(1) runs");
    String::from_utf8_lossy(&out.stdout).trim().to_lowercase()
}

pub struct Fleet {
    home: tempfile::TempDir,
    storage: tempfile::TempDir,
}

impl Fleet {
    /// This machine, declared as the only target in the registry.
    pub fn local() -> Self {
        let fleet = Self::empty();
        fleet.write_registry(&json!({
            "schema_version": 2,
            "targets": [Self::this_machine()],
            "coordinators": [],
        }));
        fleet
    }

    /// This machine, plus a service directory whose authority is a host that
    /// cannot be reached from here.
    ///
    /// The authority's ssh destination is under `.invalid`, the TLD RFC 2606
    /// reserves so that it never resolves: the absence the case needs is a
    /// name resolution that genuinely fails on this machine, and no packet
    /// leaves it. The schema requires the authority target to declare an ssh
    /// path (`service_resolution.rs`: "authority.target: must declare an SSH
    /// connection path"), which is why the unreachable host is declared here
    /// rather than merely referenced.
    pub fn with_unreachable_authority() -> Self {
        let fleet = Self::empty();
        fleet.write_registry(&json!({
            "schema_version": 2,
            "targets": [
                Self::this_machine(),
                {
                    "name": AUTHORITY,
                    "kind": "local",
                    "ssh": format!("stado@{AUTHORITY}.invalid"),
                    "release_platform": platform(),
                    "services": [
                        {"name": "brama", "kind": "launchd", "path": "/opt/stado/brama.plist"}
                    ],
                },
            ],
            "coordinators": [],
            "service_directory": {
                "authority": {"target": AUTHORITY, "command": "/opt/stado/bin/stado"},
                "generation": 7,
                "services": {
                    "brama": {
                        "managed_service": "brama",
                        "active_host": AUTHORITY,
                        "endpoints": {AUTHORITY: {"url": "http://127.0.0.1:8080"}},
                        "consumers": {"lem": {"capabilities": ["model-routing"]}},
                    }
                },
            },
        }));
        fleet
    }

    fn empty() -> Self {
        Self {
            home: tempfile::tempdir().expect("a tempdir HOME"),
            storage: tempfile::tempdir().expect("a tempdir store"),
        }
    }

    fn this_machine() -> Value {
        json!({
            "name": TARGET,
            "kind": "local",
            "release_platform": platform(),
            "hostnames": [kernel_hostname()],
        })
    }

    fn write_registry(&self, document: &Value) {
        std::fs::write(
            self.store().join("registry.json"),
            serde_json::to_string_pretty(document).expect("the registry serializes"),
        )
        .expect("the registry is written");
    }

    pub fn store(&self) -> &Path {
        self.storage.path()
    }

    /// Run the built binary against this fleet, with `extra` environment on
    /// top of the isolation every case shares.
    pub fn stado_with(&self, extra: &[(&str, &str)], args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .args(args)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            // The resolver and the host channel read ssh keys, control
            // sockets and logs out of $HOME/.stado; pointed at a tempdir they
            // cannot reach the operator's live ones.
            .env("HOME", self.home.path())
            .env("NO_COLOR", "1")
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.store())
            // A set-but-missing STADO_CONFIG disables config-file discovery.
            .env("STADO_CONFIG", self.store().join("no-such-config.json"));
        for (key, value) in extra {
            command.env(key, value);
        }
        command.output().expect("stado binary runs")
    }

    pub fn stado(&self, args: &[&str]) -> Output {
        self.stado_with(&[], args)
    }

    /// `stado host link <host> --json`, the operator surface this area is about.
    pub fn link(&self, host: &str) -> Output {
        self.stado(&["host", "link", host, "--json"])
    }

    /// [`Fleet::link`] with the operator's silence threshold overridden.
    pub fn link_with_threshold(&self, host: &str, threshold: &str) -> Output {
        self.stado_with(
            &[("STADO_SILENCE_THRESHOLD_SECONDS", threshold)],
            &["host", "link", host, "--json"],
        )
    }

    /// Publish a beacon for [`TARGET`] stamped `at`, the way the host's own
    /// publisher does, and return the instant it carries.
    ///
    /// `host_health/<slug>.json` is the path `load_host_health` reads and the
    /// only link between a registry target and its beacon; the command names
    /// the slugs it checked in its own refusal when the object is absent.
    pub fn publish_beacon(&self, at: DateTime<Utc>) -> DateTime<Utc> {
        let directory = self.store().join("host_health");
        std::fs::create_dir_all(&directory).expect("the beacon prefix is created");
        let stamp = at.to_rfc3339_opts(SecondsFormat::Millis, true);
        std::fs::write(
            directory.join(format!("{TARGET}.json")),
            serde_json::to_string_pretty(&json!({"host": TARGET, "reported_at": stamp}))
                .expect("the beacon serializes"),
        )
        .expect("the beacon is written");
        crate::report::instant(&stamp)
    }

    /// Seed one already-closed silence record, in the product's own record
    /// shape and under the key the product keys it by.
    pub fn seed_closed_silence(&self, started_at: DateTime<Utc>, ended_at: DateTime<Utc>) {
        let record = json!({
            "host": TARGET,
            "started_at": started_at.to_rfc3339_opts(SecondsFormat::Micros, true),
            "ended_at": ended_at.to_rfc3339_opts(SecondsFormat::Micros, true),
            "duration_seconds": ended_at.signed_duration_since(started_at).num_seconds(),
            "first_reader_error": Value::Null,
            "observed_by": ["cli"],
        });
        let directory = self.store().join(SILENCE_PREFIX).join(TARGET);
        std::fs::create_dir_all(&directory).expect("the silence prefix is created");
        std::fs::write(
            directory.join(format!("{}.json", blob_key(started_at))),
            serde_json::to_string_pretty(&record).expect("the record serializes"),
        )
        .expect("the record is written");
    }

    /// Re-date the refusal a command already published, keeping the document
    /// the product wrote and moving only the instant it happened at.
    pub fn redate_refusal(&self, host: &str, at: DateTime<Utc>) {
        let directory = self.store().join(REFUSAL_PREFIX).join(host);
        let names = blob_names(&directory);
        assert_eq!(names.len(), 1, "expected one published refusal: {names:?}");
        let mut record: Value = serde_json::from_str(
            &std::fs::read_to_string(directory.join(&names[0])).expect("the refusal is on disk"),
        )
        .expect("the refusal stays JSON");
        record["at"] = json!(at.to_rfc3339_opts(SecondsFormat::Micros, true));
        std::fs::remove_file(directory.join(&names[0])).expect("the old key is removed");
        std::fs::write(
            directory.join(format!("{}.json", blob_key(at))),
            serde_json::to_string_pretty(&record).expect("the refusal serializes"),
        )
        .expect("the re-dated refusal is written");
    }

    /// Blob names under one store prefix for one host, sorted.
    pub fn blobs(&self, prefix: &str, host: &str) -> Vec<String> {
        blob_names(&self.store().join(prefix).join(host))
    }

    /// The document at one blob, parsed.
    pub fn on_disk(&self, prefix: &str, host: &str, name: &str) -> Value {
        let path = self.store().join(prefix).join(host).join(name);
        let body = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{} is not on disk: {error}", path.display()));
        serde_json::from_str(&body).expect("the record stays JSON")
    }
}

fn blob_names(directory: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// The compact UTC blob key the product keys these records by.
fn blob_key(at: DateTime<Utc>) -> String {
    at.format("%Y%m%dT%H%M%S%.6fZ").to_string()
}

fn platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("linux", "x86_64") => "linux-amd64",
        (os, arch) => panic!("no release platform mapping for {os}-{arch}"),
    }
}
