//! The isolated fleet these stories read through: an authority they can
//! break on purpose, a HOME whose last-known-good copy is the test's own,
//! and the four registry documents the stories feed it.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A valid registry-v2 document with one host, distinct from every host in
/// the snapshot bundled with the binary. `stado registry validate` accepts
/// it, which is what makes it eligible for the cache.
pub(crate) const SEEDED_REGISTRY: &str = r#"{
    "schema_version": 2,
    "targets": [
        {
            "name": "w1",
            "kind": "local",
            "ssh": "u@10.0.0.1",
            "release_platform": "linux-amd64",
            "hostnames": ["w1.local"]
        }
    ],
    "coordinators": []
}"#;

/// The same document with a second host, used to prove a later successful
/// read replaces the copy and re-dates it.
pub(crate) const SEEDED_REGISTRY_GROWN: &str = r#"{
    "schema_version": 2,
    "targets": [
        {
            "name": "w1",
            "kind": "local",
            "ssh": "u@10.0.0.1",
            "release_platform": "linux-amd64",
            "hostnames": ["w1.local"]
        },
        {
            "name": "w2",
            "kind": "local",
            "ssh": "u@10.0.0.2",
            "release_platform": "linux-amd64",
            "hostnames": ["w2.local"]
        }
    ],
    "coordinators": []
}"#;

/// A document the store holds and the registry-v2 contract rejects. The
/// loader TOLERATES it — it models no targets and reports nothing — so this
/// is the document that proves the cache gate is the contract and not the
/// loader.
/// What the authority actually served on 2026-08-31 for about nine minutes.
/// Schema-valid, contract-clean, and empty.
pub(crate) const EMPTY_FLEET_REGISTRY: &str =
    r#"{"schema_version": 2, "coordinators": [], "targets": []}"#;

pub(crate) const CONTRACT_VIOLATING_REGISTRY: &str =
    r#"{"schema_version": 2, "targets": "not-a-list"}"#;

/// The authority's own words when the store cannot be read, copied from a
/// live run: `stado registry beacon-age` against a local store whose
/// `registry.json` is mode 000.
pub(crate) const UNREACHABLE_AUTHORITY: &str =
    "registry store unreachable (local:registry.json): Permission denied (os error 13)";

/// A HOME and a storage root, both temporary, plus the seeded document.
pub(crate) struct Fleet {
    home: tempfile::TempDir,
    storage: tempfile::TempDir,
}

impl Fleet {
    pub(crate) fn new(document: &str) -> Self {
        let fleet = Self {
            home: tempfile::tempdir().unwrap(),
            storage: tempfile::tempdir().unwrap(),
        };
        fleet.publish(document);
        fleet
    }

    /// Replace the document the canonical store holds.
    pub(crate) fn publish(&self, document: &str) {
        let path = self.registry_blob();
        // A previous step may have left it unreadable on purpose.
        if path.exists() {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        std::fs::write(path, document).unwrap();
    }

    pub(crate) fn registry_blob(&self) -> PathBuf {
        self.storage.path().join("registry.json")
    }

    /// Make the canonical read fail the way an unreachable store fails,
    /// without a network: the object is there and cannot be read.
    pub(crate) fn break_authority(&self) {
        std::fs::set_permissions(self.registry_blob(), std::fs::Permissions::from_mode(0o000))
            .unwrap();
    }

    /// The contract's path for the last-known-good copy.
    pub(crate) fn copy_path(&self) -> PathBuf {
        self.home
            .path()
            .join(".stado")
            .join("cache")
            .join("registry-last-good.json")
    }

    /// The contract's path for the sidecar that dates the copy.
    pub(crate) fn sidecar_path(&self) -> PathBuf {
        self.home
            .path()
            .join(".stado")
            .join("cache")
            .join("registry-last-good.meta.json")
    }

    pub(crate) fn sidecar(&self) -> serde_json::Value {
        let body = std::fs::read_to_string(self.sidecar_path()).expect("sidecar exists");
        serde_json::from_str(&body).expect("sidecar stays JSON")
    }

    /// Date the copy `seconds` in the past, the way a store that has been
    /// down for a while leaves it.
    pub(crate) fn backdate_copy(&self, seconds: i64) {
        let mut sidecar = self.sidecar();
        let read_at = chrono::Utc::now() - chrono::Duration::seconds(seconds);
        sidecar["read_at"] =
            serde_json::Value::String(read_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
        std::fs::write(self.sidecar_path(), sidecar.to_string()).unwrap();
    }

    pub(crate) fn discard_copy(&self) {
        std::fs::remove_dir_all(self.home.path().join(".stado").join("cache")).unwrap();
    }

    pub(crate) fn stado(&self, args: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_stado"));
        cmd.args(args)
            .env("HOME", self.home.path())
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.storage.path())
            // A set-but-missing STADO_CONFIG disables config-file discovery.
            .env(
                "STADO_CONFIG",
                self.storage.path().join("no-such-config.json"),
            )
            .env_remove("COMPUTE_API_KEY")
            .env_remove("COMPUTE_API_URL")
            .env_remove("WC_PROFILES_DIR");
        cmd.output().expect("stado binary runs")
    }
}

pub(crate) fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

pub(crate) fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

pub(crate) fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|exc| panic!("{}: {exc}", path.display()))
}
