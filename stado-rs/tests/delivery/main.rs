//! `stado delivery`: what a session records when it has pushed, and what the
//! fleet refuses.
//!
//! The built binary drives an isolated canonical registry — its own HOME, its
//! own local storage — so nothing here touches the operator's fleet. No
//! qualification pass is started: a pass submits a real build, and these
//! cases are about the record and the refusals that guard it, including the
//! one that stops a pass from spending a build on nobody.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};

const PRODUCT: &str = "delivery-test-product";
const SOURCE: &str = "https://github.com/wisent-ai/stado.git";
/// A full commit is forty hexadecimal characters.
const REVISION: &str = "1111111111111111111111111111111111111111";
const SECOND_REVISION: &str = "2222222222222222222222222222222222222222";
const TASK: &str = "task-1234567890abcdef";

struct Fleet {
    home: tempfile::TempDir,
    storage: PathBuf,
}

impl Fleet {
    fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/delivery-runs");
        fs::create_dir_all(&root).unwrap();
        let home = tempfile::Builder::new()
            .prefix("delivery-")
            .tempdir_in(root)
            .unwrap();
        let storage = home.path().join("store");
        fs::create_dir_all(&storage).unwrap();
        let registry = json!({
            "schema_version": schema_version(),
            "targets": [],
            "coordinators": []
        });
        fs::write(
            storage.join("registry.json"),
            serde_json::to_string_pretty(&registry).unwrap(),
        )
        .unwrap();
        let fleet = Self { home, storage };
        fleet.declare_recipe();
        fleet
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .env_clear()
            .env("HOME", self.home.path())
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("STADO_CONFIG", self.home.path().join("no-config.json"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &self.storage)
            .env("WC_PROVIDERS", "local")
            .env("WC_VAST_AUTO_LIST", "false");
        command
    }

    fn invoke(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }

    fn invoke_ok(&self, args: &[&str]) -> Output {
        let output = self.invoke(args);
        assert!(
            output.status.success(),
            "stado {} failed\nstdout:\n{}\nstderr:\n{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        output
    }

    fn declare_recipe(&self) {
        self.invoke_ok(&[
            "builds",
            "add",
            "--name",
            PRODUCT,
            "--repo",
            SOURCE,
            "--branch",
            "main",
            "--command",
            "true",
            "--artifact",
            "out",
            "--platform",
            "darwin-arm64",
        ]);
    }

    fn deliver(&self, revision: &str, summary: &str) -> Output {
        self.invoke(&[
            "delivery",
            "deliver",
            "--product",
            PRODUCT,
            "--revision",
            revision,
            "--summary",
            summary,
            "--task",
            TASK,
        ])
    }

    fn pending(&self) -> Value {
        let output = self.invoke_ok(&["delivery", "pending", "--json"]);
        serde_json::from_slice(&output.stdout).unwrap()
    }
}

/// The canonical registry document's own schema version, as every other
/// fixture in this suite writes it.
fn schema_version() -> u64 {
    2
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

/// A delivery is a record, not a build: it names a product some recipe can
/// build, an exact revision, and the task it answers. Each of those is
/// refused when it is not true, because a delivery nothing can qualify waits
/// forever, and waiting forever reads the same as being forgotten.
#[test]
fn a_delivery_names_a_product_that_can_be_built_and_an_exact_revision() {
    let fleet = Fleet::new();

    let unknown = fleet.invoke(&[
        "delivery",
        "deliver",
        "--product",
        "no-such-product",
        "--revision",
        REVISION,
    ]);
    assert!(!unknown.status.success());
    assert!(
        stderr(&unknown).contains("no build recipe named"),
        "{}",
        stderr(&unknown)
    );

    let short = fleet.invoke(&[
        "delivery",
        "deliver",
        "--product",
        PRODUCT,
        "--revision",
        "1111111",
    ]);
    assert!(!short.status.success());
    assert!(
        stderr(&short).contains("must be the exact full commit"),
        "{}",
        stderr(&short)
    );

    let first = fleet.deliver(REVISION, "first change");
    assert!(first.status.success(), "{}", stderr(&first));

    let again = fleet.deliver(REVISION, "first change");
    assert!(!again.status.success());
    assert!(
        stderr(&again).contains("already delivered"),
        "{}",
        stderr(&again)
    );
}

/// What waits is readable, in the order it was delivered, with the task each
/// revision answers — that list is the whole point of separating writing from
/// building.
#[test]
fn pending_lists_what_has_been_written_and_not_yet_proven() {
    let fleet = Fleet::new();
    assert_eq!(fleet.pending(), json!([]));

    fleet.deliver(REVISION, "first change");
    fleet.deliver(SECOND_REVISION, "second change");

    let pending = fleet.pending();
    let rows = pending.as_array().expect("pending is an array");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["revision"], REVISION);
    assert_eq!(rows[0]["state"], "waiting");
    assert_eq!(rows[0]["task"], TASK);
    assert_eq!(rows[1]["summary"], "second change");

    let failures = fleet.invoke_ok(&["delivery", "failures", "--json"]);
    assert_eq!(
        serde_json::from_slice::<Value>(&failures.stdout).unwrap(),
        json!([]),
        "nothing has failed yet"
    );
}

/// A pass costs a build, so it is refused when there is nothing to prove and
/// when the fleet's daily ceiling is spent. Both refusals say what to do
/// instead, because a refusal that names no route is the one that gets walked
/// around.
#[test]
fn a_pass_is_refused_with_nothing_waiting_and_when_the_days_builds_are_spent() {
    let fleet = Fleet::new();

    let empty = fleet.invoke(&[
        "delivery",
        "qualify",
        "--product",
        PRODUCT,
        "--run-id",
        "pass-empty",
    ]);
    assert!(!empty.status.success());
    assert!(
        stderr(&empty).contains("nothing is waiting"),
        "{}",
        stderr(&empty)
    );
    assert!(
        stderr(&empty).contains("stado delivery deliver"),
        "{}",
        stderr(&empty)
    );

    fleet.deliver(REVISION, "first change");
    fleet.invoke_ok(&["builds", "budget", "--limit", "0"]);

    let spent = fleet.invoke(&[
        "delivery",
        "qualify",
        "--product",
        PRODUCT,
        "--run-id",
        "pass-spent",
    ]);
    assert!(!spent.status.success());
    assert!(
        stderr(&spent).contains("daily build budget is spent"),
        "{}",
        stderr(&spent)
    );
    assert_eq!(
        fleet.pending().as_array().map(Vec::len),
        Some(1),
        "a refused pass leaves the delivery waiting, not qualifying"
    );
}
