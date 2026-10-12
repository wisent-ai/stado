//! `stado quality check` runs the gates a manifest declares of kind `format`,
//! whatever they are named, and no other kind: a release build runs every
//! gate, the pre-push check only those that read the committed bytes.
//!
//! One isolated deployment per case: a local store, a committed product
//! checkout and the real `stado` executable. Expected: a manifest whose gate
//! named `fmt` is of kind `lint` and exits non-zero, beside a gate named
//! `style` of kind `format` that passes, passes the check and reports the
//! `style` gate — the name selects nothing; a manifest whose only gates are
//! `lint` and `test` is refused naming the kind it lacks and the kinds it
//! declares; a manifest whose gate declares no kind is refused before any
//! gate runs, naming the missing field. The checkout commits with the
//! identity of the account running the test, and its manifest takes its
//! schema version from Stado's own manifest.
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
fn the_check_runs_format_gates_by_kind_and_refuses_a_manifest_without_one() {
    let mut deployment = Deployment::start();
    let initialised = deployment.run(&["config", "init"]);
    assert!(
        initialised.status.success(),
        "config init: {}",
        String::from_utf8_lossy(&initialised.stderr)
    );

    // A failing gate named `fmt` of kind `lint` is not the check's to run;
    // the passing `style` gate of kind `format` is.
    let by_kind = deployment.checkout(
        "by-kind",
        json!([
            {"name": "fmt", "kind": "lint", "argv": ["false"]},
            {"name": "style", "kind": "format", "argv": ["cat", "VERSION"]}
        ]),
    );
    let passed = deployment.run(&["quality", "check", "--root", &by_kind]);
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&passed.stdout),
        String::from_utf8_lossy(&passed.stderr)
    );
    assert!(
        passed.status.success(),
        "the check ran a gate that is not of kind format: {said}"
    );
    assert!(
        said.contains("stado quality check: cat VERSION")
            && !said.contains("stado quality check: false"),
        "the check did not report the format gate alone: {said}"
    );

    // Gates of other kinds alone leave the check nothing to run.
    let no_format = deployment.checkout(
        "no-format",
        json!([
            {"name": "fmt", "kind": "lint", "argv": ["cat", "VERSION"]},
            {"name": "journey", "kind": "test", "argv": ["cat", "VERSION"]}
        ]),
    );
    let refused = deployment.run(&["quality", "check", "--root", &no_format]);
    let said = String::from_utf8_lossy(&refused.stderr).into_owned();
    assert!(!refused.status.success(), "a manifest without a format gate passed");
    assert!(
        said.contains("declares no gate of kind format for platform linux-amd64")
            && said.contains("fmt (lint)")
            && said.contains("journey (test)"),
        "the refusal does not name the missing kind and the declared gates: {said}"
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
