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
            "hostnames": ["w1.local"],
            "slots": 1
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
            "hostnames": ["w1.local"],
            "slots": 1
        },
        {
            "name": "w2",
            "kind": "local",
            "ssh": "u@10.0.0.2",
            "release_platform": "linux-amd64",
            "hostnames": ["w2.local"],
            "slots": 1
        }
    ],
    "coordinators": []
}"#;

/// A document the store holds and the registry-v2 contract rejects. The
/// loader TOLERATES it — it models no targets and reports nothing — so this
/// is the document that proves the cache gate is the contract and not the
/// loader.
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
        std::fs::set_permissions(
            self.registry_blob(),
            std::fs::Permissions::from_mode(0o000),
        )
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
        sidecar["read_at"] = serde_json::Value::String(
            read_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        );
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
            .env("STADO_CONFIG", self.storage.path().join("no-such-config.json"))
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

/// `stado registry beacon-age` is the reader used throughout: it prints one
/// row per registry host and nothing else, so which document answered is
/// visible in stdout without depending on the machine the test runs on.
#[test]
fn a_canonical_read_records_the_copy_and_dates_it() {
    let fleet = Fleet::new(SEEDED_REGISTRY);

    let out = fleet.stado(&["registry", "beacon-age"]);
    assert!(out.status.success(), "read failed: {}", stderr(&out));
    assert!(stdout(&out).contains("w1"), "got: {}", stdout(&out));

    // The copy is the document, byte for byte: a reader hands it back to the
    // same loader, so anything this side rewrote would be a second opinion.
    assert_eq!(read(&fleet.copy_path()), SEEDED_REGISTRY);

    let sidecar = fleet.sidecar();
    let read_at = sidecar["read_at"].as_str().expect("read_at is a string");
    let read_at = chrono::DateTime::parse_from_rfc3339(read_at)
        .unwrap_or_else(|exc| panic!("read_at {read_at} is not RFC3339: {exc}"));
    let age = (chrono::Utc::now() - read_at.with_timezone(&chrono::Utc)).num_seconds();
    assert!((0..60).contains(&age), "read_at is {age}s old");
    let first_generation = sidecar["generation"]
        .as_str()
        .expect("generation is a string")
        .to_string();
    assert!(!first_generation.is_empty(), "generation is empty");

    // EVERY successful read writes both files, not just the first: the
    // authority publishes a second host and the copy follows, with a new
    // generation and a fresh date.
    fleet.publish(SEEDED_REGISTRY_GROWN);
    let out = fleet.stado(&["registry", "beacon-age"]);
    assert!(out.status.success(), "second read failed: {}", stderr(&out));
    assert!(stdout(&out).contains("w2"), "got: {}", stdout(&out));
    assert_eq!(read(&fleet.copy_path()), SEEDED_REGISTRY_GROWN);
    assert_ne!(
        fleet.sidecar()["generation"].as_str().unwrap(),
        first_generation,
        "generation did not follow the document"
    );
}

#[test]
fn an_unreachable_authority_serves_the_copy_and_names_its_age() {
    let fleet = Fleet::new(SEEDED_REGISTRY);
    assert!(fleet.stado(&["registry", "beacon-age"]).status.success());

    // Seven minutes of silence, then the reader is asked again.
    fleet.backdate_copy(412);
    fleet.break_authority();
    let out = fleet.stado(&["registry", "beacon-age"]);

    // The command answers instead of dying with the store, and answers from
    // the copy: the seeded host is still there.
    assert!(out.status.success(), "read refused: {}", stderr(&out));
    assert!(stdout(&out).contains("w1"), "got: {}", stdout(&out));

    // One sentence, naming the age of what is being read and keeping the
    // authority's own words. The second is off by one when the truncated
    // read_at and the current second fall either side of a tick.
    let complaint = stderr(&out);
    assert!(
        complaint.contains("reading the last-known-good registry copy from 412s ago")
            || complaint.contains("reading the last-known-good registry copy from 413s ago"),
        "no age in: {complaint}"
    );
    assert!(
        complaint.contains(&fleet.copy_path().display().to_string()),
        "no path in: {complaint}"
    );
    assert!(
        complaint.contains(UNREACHABLE_AUTHORITY),
        "the authority's own error is missing from: {complaint}"
    );

    // Falling back never rewrites the copy — the age an operator reads is
    // the age of the document, not of the last attempt to reach the store.
    assert_eq!(read(&fleet.copy_path()), SEEDED_REGISTRY);

    // With no copy at all the refusal is the one it always was, and it is a
    // refusal: a reader that cannot reach the authority and has nothing on
    // disk must not answer from thin air.
    fleet.discard_copy();
    let out = fleet.stado(&["registry", "beacon-age"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains(&format!("Error: {UNREACHABLE_AUTHORITY}")),
        "got: {}",
        stderr(&out)
    );
}

#[test]
fn a_document_that_fails_the_contract_is_never_recorded() {
    let fleet = Fleet::new(CONTRACT_VIOLATING_REGISTRY);

    // The loader tolerates the document, so the command succeeds with an
    // empty fleet — that behavior is not what is under test here. What is:
    // nothing was recorded, and the reason was said out loud.
    let out = fleet.stado(&["registry", "beacon-age"]);
    assert!(out.status.success(), "read failed: {}", stderr(&out));
    assert!(
        stderr(&out).contains(&format!(
            "[registry-cache] not recording the last-known-good registry in {}: registry.targets: must be an array",
            fleet.copy_path().display()
        )),
        "got: {}",
        stderr(&out)
    );
    assert!(!fleet.copy_path().exists(), "a rejected document was cached");
    assert!(!fleet.sidecar_path().exists(), "a rejected document was dated");

    // And it cannot overwrite a copy that IS good: the whole value of the
    // cache is that the document in it was known good once.
    fleet.publish(SEEDED_REGISTRY);
    assert!(fleet.stado(&["registry", "beacon-age"]).status.success());
    let dated = fleet.sidecar();
    fleet.publish(CONTRACT_VIOLATING_REGISTRY);
    assert!(fleet.stado(&["registry", "beacon-age"]).status.success());
    assert_eq!(read(&fleet.copy_path()), SEEDED_REGISTRY);
    assert_eq!(fleet.sidecar(), dated);
}

/// `stado identity list --json` is the reader for this one: it goes through
/// the "auto" chain (authority, then copy, then bundle), and the bundled
/// snapshot declares identity bindings while the seeded document declares
/// none, so which of the three answered is visible in stdout.
#[test]
fn the_bundled_snapshot_is_the_last_resort_below_the_copy() {
    let fleet = Fleet::new(SEEDED_REGISTRY);
    let out = fleet.stado(&["identity", "list", "--json"]);
    assert!(out.status.success(), "read failed: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "[]", "the authority did not answer");

    // Authority down, copy present: the copy wins, and the bundle is not
    // touched — no bundled host appears in the answer.
    fleet.break_authority();
    let out = fleet.stado(&["identity", "list", "--json"]);
    assert!(out.status.success(), "read refused: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "[]", "the copy did not answer");
    assert!(
        stderr(&out).contains("reading the last-known-good registry copy from"),
        "got: {}",
        stderr(&out)
    );
    assert!(
        !stdout(&out).contains("control-host"),
        "the bundled snapshot answered over a usable copy: {}",
        stdout(&out)
    );

    // Authority down and no copy: only now is the bundle read, and the
    // sentence says so, because a snapshot as old as the binary answering in
    // silence is how a decommissioned host stays declared for a fortnight.
    fleet.discard_copy();
    let out = fleet.stado(&["identity", "list", "--json"]);
    assert!(out.status.success(), "read refused: {}", stderr(&out));
    assert!(
        stdout(&out).contains("control-host"),
        "the bundled snapshot did not answer: {}",
        stdout(&out)
    );
    assert!(
        stderr(&out).contains(&format!(
            "reading the registry snapshot bundled with this binary because the authority did not answer and there is no last-known-good copy at {}: {UNREACHABLE_AUTHORITY}",
            fleet.copy_path().display()
        )),
        "got: {}",
        stderr(&out)
    );
}
