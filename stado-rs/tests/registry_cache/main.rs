//! Reader-side registry cache tests against the local storage backend.
//!
//! Every test drives the built `stado` binary (`CARGO_BIN_EXE_stado`) with
//! WC_STORAGE_BACKEND=local + WC_LOCAL_STORAGE_PATH=<TempDir> and a
//! STADO_CONFIG pointing at a nonexistent path, so the developer's real
//! config can never leak into a test.
//!
//! HOME is a tempdir too, and that one is not optional: the last-known-good
//! copy lives at `$HOME/.stado/cache/registry-last-good.json`, so a test that
//! isolates only the storage backend writes the operator's real cache and a
//! later outage serves the fleet a toy registry. That happened on
//! 2026-08-19 — the live copy was found holding a two-line fake document with
//! one target — which is why the paths below are spelled out literally rather
//! than read back from the code under test.
//!
//! What is defended here: a successful canonical read records the copy and
//! dates it, an unreachable authority serves that copy and names its age in
//! the sentence the operator sees, a document that fails the registry-v2
//! contract is never recorded, and the snapshot bundled with the binary is
//! reached only when the authority AND the copy are both gone.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A valid registry-v2 document with one host, distinct from every host in
/// the snapshot bundled with the binary. `stado registry validate` accepts
/// it, which is what makes it eligible for the cache.
const SEEDED_REGISTRY: &str = r#"{
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
const SEEDED_REGISTRY_GROWN: &str = r#"{
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
const EMPTY_FLEET_REGISTRY: &str = r#"{"schema_version": 2, "coordinators": [], "targets": []}"#;

const CONTRACT_VIOLATING_REGISTRY: &str = r#"{"schema_version": 2, "targets": "not-a-list"}"#;

/// The authority's own words when the store cannot be read, copied from a
/// live run: `stado registry beacon-age` against a local store whose
/// `registry.json` is mode 000.
const UNREACHABLE_AUTHORITY: &str =
    "registry store unreachable (local:registry.json): Permission denied (os error 13)";

/// A HOME and a storage root, both temporary, plus the seeded document.
struct Fleet {
    home: tempfile::TempDir,
    storage: tempfile::TempDir,
}

impl Fleet {
    fn new(document: &str) -> Self {
        let fleet = Self {
            home: tempfile::tempdir().unwrap(),
            storage: tempfile::tempdir().unwrap(),
        };
        fleet.publish(document);
        fleet
    }

    /// Replace the document the canonical store holds.
    fn publish(&self, document: &str) {
        let path = self.registry_blob();
        // A previous step may have left it unreadable on purpose.
        if path.exists() {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        std::fs::write(path, document).unwrap();
    }

    fn registry_blob(&self) -> PathBuf {
        self.storage.path().join("registry.json")
    }

    /// Make the canonical read fail the way an unreachable store fails,
    /// without a network: the object is there and cannot be read.
    fn break_authority(&self) {
        std::fs::set_permissions(self.registry_blob(), std::fs::Permissions::from_mode(0o000))
            .unwrap();
    }

    /// The contract's path for the last-known-good copy.
    fn copy_path(&self) -> PathBuf {
        self.home
            .path()
            .join(".stado")
            .join("cache")
            .join("registry-last-good.json")
    }

    /// The contract's path for the sidecar that dates the copy.
    fn sidecar_path(&self) -> PathBuf {
        self.home
            .path()
            .join(".stado")
            .join("cache")
            .join("registry-last-good.meta.json")
    }

    fn sidecar(&self) -> serde_json::Value {
        let body = std::fs::read_to_string(self.sidecar_path()).expect("sidecar exists");
        serde_json::from_str(&body).expect("sidecar stays JSON")
    }

    /// Date the copy `seconds` in the past, the way a store that has been
    /// down for a while leaves it.
    fn backdate_copy(&self, seconds: i64) {
        let mut sidecar = self.sidecar();
        let read_at = chrono::Utc::now() - chrono::Duration::seconds(seconds);
        sidecar["read_at"] =
            serde_json::Value::String(read_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
        std::fs::write(self.sidecar_path(), sidecar.to_string()).unwrap();
    }

    fn discard_copy(&self) {
        std::fs::remove_dir_all(self.home.path().join(".stado").join("cache")).unwrap();
    }

    fn stado(&self, args: &[&str]) -> Output {
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

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|exc| panic!("{}: {exc}", path.display()))
}


mod cases;
