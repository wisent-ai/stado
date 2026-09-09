//! `stado space report`, driven as the real binary against an isolated
//! registry that names this machine, so the read executes locally.
//!
//! What this defends is one shape of failure. The report is two things at
//! once: three cheap fields — free space, the janitor's last outcome, memory —
//! and one attribution walk over the whole selected tree. The walk cost more
//! than the shared two-minute channel bound, so on 2026-09-02 and again on
//! 2026-09-09 the command died having computed nothing, on the very machine
//! whose disk was the question. The cheap fields cost under a second and were
//! lost with it.
//!
//! So the walk now has its own budget, and exceeding it is reported rather
//! than fatal. The budget is forced to one second here, which is the only way
//! to reach that branch without a host large enough to be slow.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::json;

/// The isolated registry names one target, and the assertions read it back.
const TARGET: &str = "space-report-fixture";

/// Directories a Mac always has, so the fixture's `PATH` finds `df`, `tr` and
/// the shell the remote program runs under.
const SYSTEM_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

/// Schema versions the product's own writers use: `config_file` for the
/// configuration document and `targets::REGISTRY_SCHEMA_VERSION` for the
/// registry. Named here rather than spelled inside the fixture documents, so
/// a reader can see which contract each number belongs to.
const CONFIG_SCHEMA_VERSION: i64 = 1;
const REGISTRY_SCHEMA_VERSION: i64 = 2;

struct Fixture {
    root: PathBuf,
    home: PathBuf,
    storage: PathBuf,
    config: PathBuf,
}

fn write_private(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).expect("write fixture file");
    let mut permissions = fs::metadata(path).expect("stat fixture file").permissions();
    permissions.set_mode(0o600);
    fs::set_permissions(path, permissions).expect("restrict fixture file");
}

fn hostname() -> String {
    let output = Command::new("/bin/hostname")
        .output()
        .expect("read this machine's hostname");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "stado-space-report-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|value| value.as_nanos())
                .unwrap_or_default()
        ));
        let home = root.join("home");
        let storage = root.join("storage");
        for directory in [&home, &storage, &root.join("tmp")] {
            fs::create_dir_all(directory).expect("create fixture directory");
        }
        let config = root.join("config.json");
        write_private(
            &config,
            &serde_json::to_vec_pretty(&json!({
                "schema_version": CONFIG_SCHEMA_VERSION,
                "storage": {"backend": "local", "local": {"path": storage}},
            }))
            .expect("render fixture config"),
        );
        write_private(
            &storage.join("registry.json"),
            &serde_json::to_vec_pretty(&json!({
                "schema_version": REGISTRY_SCHEMA_VERSION,
                "targets": [{
                    "name": TARGET,
                    "kind": "local",
                    "ssh": null,
                    "release_platform": "darwin-arm64",
                    "hostnames": [hostname()],
                    "services": [],
                }],
                "coordinators": [],
            }))
            .expect("render fixture registry"),
        );
        Self {
            root,
            home,
            storage,
            config,
        }
    }

    /// The report with the attribution walk held to `budget_seconds`.
    fn report(&self, budget_seconds: &str, extra: &[&str]) -> Output {
        let mut args = vec!["space", "report", TARGET];
        args.extend_from_slice(extra);
        Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(&args)
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", SYSTEM_PATH)
            .env("TMPDIR", self.root.join("tmp"))
            .env("STADO_CONFIG", &self.config)
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &self.storage)
            .env("WC_PROVIDERS", "local")
            .env("NO_COLOR", "1")
            .env("STADO_INVENTORY_BUDGET_SECONDS", budget_seconds)
            .output()
            .expect("run stado space report")
    }

    fn cleanup(&self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn a_walk_that_exceeds_its_budget_still_reports_free_space_and_the_janitor() {
    let fixture = Fixture::new();
    let output = fixture.report("1", &[]);
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    assert_eq!(
        output.status.code(),
        Some(0),
        "a slow walk is not a failed command; stderr: {stderr}"
    );
    assert!(
        text.contains("disk:") && text.contains("free:") && text.contains("GiB"),
        "the disk and free-space lines survive the slow walk: {text}"
    );
    assert!(
        text.contains("memory:"),
        "the memory reading survives the slow walk: {text}"
    );
    assert!(
        text.contains("janitor:"),
        "the janitor's own outcome survives the slow walk: {text}"
    );
    fixture.cleanup();
}

#[test]
fn the_report_names_the_walk_it_could_not_finish() {
    let fixture = Fixture::new();
    let output = fixture.report("1", &["--json"]);
    let document: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("the report is one JSON document");
    let detail = document
        .get("inventory_incomplete")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("the report names the unfinished walk: {document}"));

    assert!(
        detail.contains("did not finish within 1 seconds"),
        "the detail carries the budget it exceeded: {detail}"
    );
    fixture.cleanup();
}
