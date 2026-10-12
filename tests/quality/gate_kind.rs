//! `stado quality check` (the pre-push check) runs the gates a product
//! installation runs: every declared gate, each kind — `format`, `lint` and
//! `test` — so a commit the push passes is not refused by its installation.
//!
//! One isolated deployment per case: a local store, a committed product
//! checkout and the real `stado` executable. Expected: a manifest whose
//! `lint` gate exits non-zero beside a passing `format` gate is refused,
//! naming the failing gate, after the format gate ran; a manifest whose
//! `lint` and `test` gates pass passes the check and reports both; a
//! manifest whose gate declares no kind is refused before any gate runs,
//! naming the missing field. The checkout commits with the identity of the
//! account running the test, and its manifest takes its schema version from
//! Stado's own manifest.
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const PRODUCT: &str = "quality-gate-kind-test";
const MANIFEST: &str = ".wisent-release.json";

struct Deployment {
    repository: PathBuf,
    root: PathBuf,
    report: Value,
}

impl Deployment {
    fn start() -> Self {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let root = repository
            .join(".build/quality-gate-kind")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(root.join("tmp")).unwrap();
        fs::create_dir_all(root.join("store")).unwrap();
        let source_revision = git_output(&repository, &["rev-parse", "HEAD"]);
        Self {
            repository,
            root,
            report: json!({
                "source_revision": source_revision,
                "commands": [],
                "outcome": "failed",
            }),
        }
    }

    fn run(&mut self, args: &[&str]) -> Output {
        let output = Command::new(env!("CARGO_BIN_EXE_stado"))
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", &self.root)
            .env("STADO_CONFIG", self.root.join(".stado").join("config.json"))
            .env("TMPDIR", self.root.join("tmp"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.root.join("store"))
            .stdin(Stdio::null())
            .args(args)
            .output()
            .unwrap();
        self.report["commands"].as_array_mut().unwrap().push(json!({
            "args": args,
            "exit_status": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        }));
        self.save();
        output
    }

    fn save(&self) {
        fs::write(
            self.root.join("report.json"),
            serde_json::to_vec_pretty(&self.report).unwrap(),
        )
        .unwrap();
    }

    /// A committed product checkout named `name` whose one platform declares
    /// `gates`.
    fn checkout(&self, name: &str, gates: Value) -> String {
        let checkout = self.root.join(name);
        fs::create_dir_all(&checkout).unwrap();
        git(&checkout, &["init", "--quiet", "--initial-branch=main"]);
        let own: Value =
            serde_json::from_slice(&fs::read(self.repository.join(MANIFEST)).unwrap()).unwrap();
        fs::write(checkout.join("VERSION"), "0.1.0\n").unwrap();
        let manifest = json!({
            "schema_version": own["schema_version"],
            "product": PRODUCT,
            "releases": true,
            "version_source": {"kind": "text", "path": "VERSION"},
            "platforms": {
                "linux-amd64": {
                    "runner_platform": "linux-amd64",
                    "quality": gates,
                    "build": {"argv": ["cat", "VERSION"]},
                    "stage": {"VERSION": "VERSION"}
                }
            },
            "promotion": {"channels": ["candidate"], "reconcile": false}
        });
        fs::write(
            checkout.join(MANIFEST),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        git(&checkout, &["add", "VERSION", MANIFEST]);
        git(&checkout, &["commit", "--quiet", "-m", "declare the gates"]);
        checkout.to_string_lossy().into_owned()
    }
}

fn git(directory: &Path, args: &[&str]) {
    let status = Command::new("git")
        .env_remove("GIT_ALLOW_PROTOCOL")
        .args(args)
        .current_dir(directory)
        .stdout(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} in {}", directory.display());
}

fn git_output(directory: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(directory)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[test]
fn the_check_runs_every_gate_the_installation_runs() {
    let mut deployment = Deployment::start();
    let initialised = deployment.run(&["config", "init"]);
    assert!(
        initialised.status.success(),
        "config init: {}",
        String::from_utf8_lossy(&initialised.stderr)
    );

    // A failing `lint` gate is the installation's to run, so the check runs
    // it too and refuses, after the passing `format` gate.
    let lint_fails = deployment.checkout(
        "lint-fails",
        json!([
            {"name": "style", "kind": "format", "argv": ["cat", "VERSION"]},
            {"name": "clippy", "kind": "lint", "argv": ["false"]}
        ]),
    );
    let refused = deployment.run(&["quality", "check", "--root", &lint_fails]);
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&refused.stdout),
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(
        !refused.status.success(),
        "the check passed a commit whose lint gate the installation refuses: {said}"
    );
    assert!(
        said.contains("stado quality check: cat VERSION")
            && said.contains("stado quality check: false"),
        "the check did not run the format gate and then the failing lint gate: {said}"
    );

    // Gates of other kinds, passing, are run and pass.
    let lint_and_test = deployment.checkout(
        "lint-and-test",
        json!([
            {"name": "check", "kind": "lint", "argv": ["cat", "VERSION"]},
            {"name": "journey", "kind": "test", "argv": ["test", "-f", "VERSION"]}
        ]),
    );
    let passed = deployment.run(&["quality", "check", "--root", &lint_and_test]);
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&passed.stdout),
        String::from_utf8_lossy(&passed.stderr)
    );
    assert!(
        passed.status.success(),
        "a commit whose lint and test gates pass was refused: {said}"
    );
    assert!(
        said.contains("stado quality check: cat VERSION")
            && said.contains("stado quality check: test -f VERSION"),
        "the check did not run the lint and the test gate: {said}"
    );

    // A gate without a kind is refused before any gate runs.
    let no_kind = deployment.checkout(
        "no-kind",
        json!([{"name": "fmt", "argv": ["cat", "VERSION"]}]),
    );
    let refused = deployment.run(&["quality", "check", "--root", &no_kind]);
    let said = String::from_utf8_lossy(&refused.stderr).into_owned();
    assert!(!refused.status.success(), "a gate without a kind passed");
    assert!(
        said.contains("kind"),
        "the refusal does not name the missing field: {said}"
    );

    deployment.report["outcome"] = json!("passed");
    deployment.save();
    eprintln!("report: {}", deployment.root.join("report.json").display());
}
