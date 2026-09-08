//! The documents these cases judge, and the temporary home the no-write case
//! owns.
//!
//! Split out of `main.rs` so each file stays inside the three hundred line
//! limit this repository enforces on itself.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use serde_json::Value;

/// The host row this process answers for. It is declaration data: no case
/// contacts it, and the subject here is which document a build accepts.
pub const LOCAL: &str = "skew-local-host";
/// A second declared row, which no process here can ask.
pub const REMOTE: &str = "skew-peer-host";

pub const REFUSES: &str = "build-refuses-registry";
pub const UNREAD: &str = "unread-build-verdict";

/// The field name the refusal fixture adds, so a case can assert the sentence
/// names what was refused. Spelled the way a newer document would spell a day
/// bound: no build in the fleet implements it, and the schema says so.
pub const UNIMPLEMENTED_FIELD: &str = "min_age_days";

/// Serializes `HOME`, which the no-write case has to own exclusively.
static HOME_LOCK: Mutex<()> = Mutex::new(());

/// A cache location this test owns, with `HOME` pointed at it for as long as
/// the guard lives. Same shape as `tests/registry_cache_refusal`, for the same
/// reason: `HOME` decides the cache location and is process-wide.
pub struct Home {
    _lock: MutexGuard<'static, ()>,
    dir: tempfile::TempDir,
    previous: Option<std::ffi::OsString>,
}

impl Home {
    pub fn new() -> Self {
        let lock = HOME_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().expect("temp HOME");
        let previous = std::env::var_os("HOME");
        std::env::set_var("HOME", dir.path());
        Self {
            _lock: lock,
            dir,
            previous,
        }
    }

    pub fn cache(&self) -> PathBuf {
        self.dir.path().join(".stado").join("cache")
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(previous) => std::env::set_var("HOME", previous),
            None => std::env::remove_var("HOME"),
        }
    }
}

/// One declared host, the full `disk_cleanup` field set, one cleaner this
/// build implements. `validate_registry` accepts it, which is what makes every
/// mutation below attributable to the mutation.
pub fn accepted() -> Value {
    serde_json::from_str(
        r#"{
        "schema_version": 2,
        "coordinators": [],
        "targets": [
            {
                "name": "skew-local-host",
                "kind": "local",
                "ssh": "u@10.0.0.1",
                "release_platform": "darwin-arm64",
                "hostnames": ["skew-local-host.local"],
                "disk_cleanup": {
                    "mode": "report",
                    "check_interval_seconds": 3600,
                    "low_free_gb": 100,
                    "target_free_gb": 200,
                    "max_bytes_per_pass": 68719476736,
                    "max_items_per_pass": 512,
                    "max_scan_items": 4096,
                    "cleaners": { "build_caches": { "min_age_seconds": 86400 } }
                }
            }
        ]
    }"#,
    )
    .expect("fixture parses")
}

/// The same document with one field no build implements inside a cleaner every
/// build knows.
///
/// This used to declare an unknown cleaner *name*, and that stopped being a
/// refusal on purpose: on 2026-09-04 refusing a whole policy for one
/// unfamiliar name switched every cleaner off on charless-mac-mini the moment
/// `release_store` was declared for a binary still queued to reach it, so an
/// unknown name is now skipped and reported by the janitor instead. A cleaner's
/// own key set is still held to the schema, which is where a document a build
/// cannot model is still refused — and refused with the name in the sentence,
/// which is what these cases read.
pub fn declares_a_field_no_build_implements() -> Value {
    let mut document = accepted();
    document["targets"][0]["disk_cleanup"]["cleaners"]["build_caches"][UNIMPLEMENTED_FIELD] =
        serde_json::json!(86400);
    document
}

/// The same document declaring a cleaner name this build does not know, which
/// a newer build will: skipped on purpose rather than refused.
pub fn newer_cleaner_declaration() -> Value {
    let mut document = accepted();
    document["targets"][0]["disk_cleanup"]["cleaners"]["cleaner_a_newer_build_knows"] =
        serde_json::json!({ "min_age_seconds": 86400 });
    document
}
