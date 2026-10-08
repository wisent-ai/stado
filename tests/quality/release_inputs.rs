//! `stado quality check`, which `stado release changes submit` runs before it
//! accepts a commit, refuses a manifest whose pinned release input is not in
//! its store, and accepts the same commit once the object is stored.
//!
//! One isolated deployment: a local store, a product checkout with an origin
//! it fetches from, and the real `stado` executable. The manifest pins one
//! content-addressed source archive. The first check runs before the archive
//! is stored and must refuse naming the input and the pin; `stado storage put`
//! then stores it, and the second check must pass and say the inputs were
//! found. The checkout commits with the identity of the account running the
//! test, and its manifest takes its schema version from Stado's own manifest.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const PRODUCT: &str = "quality-inputs-test";
const INPUT: &str = "archive";
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
            .join(".build/quality-release-inputs")
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

    fn cli(&mut self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "stado {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn save(&self) {
        fs::write(
            self.root.join("report.json"),
            serde_json::to_vec_pretty(&self.report).unwrap(),
        )
        .unwrap();
    }

    /// A product checkout pinning `uri`, committed and pushed to its own
    /// origin.
    fn checkout(&self, uri: &str, sha256: &str) -> PathBuf {
        let origin = self.root.join("origin.git");
        let checkout = self.root.join("checkout");
        fs::create_dir_all(&origin).unwrap();
        fs::create_dir_all(&checkout).unwrap();
        git(
            &origin,
            &["init", "--quiet", "--bare", "--initial-branch=main"],
        );
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
                    "quality": [{"name": "fmt", "argv": ["cat", "VERSION"]}],
                    "build": {"argv": ["cat", "VERSION"]},
                    "stage": {"VERSION": "VERSION"}
                }
            },
            "promotion": {"channels": ["candidate"], "reconcile": false},
            "inputs": {
                INPUT: {"uri": uri, "sha256": sha256, "mount": INPUT, "extract": false}
            }
        });
        fs::write(
            checkout.join(MANIFEST),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        git(&checkout, &["add", "VERSION", MANIFEST]);
        git(&checkout, &["commit", "--quiet", "-m", "pin the archive"]);
        git(
            &checkout,
            &["remote", "add", "origin", &origin.to_string_lossy()],
        );
        git(&checkout, &["push", "--quiet", "origin", "main"]);
        checkout
    }
}

fn git(directory: &Path, args: &[&str]) {
    // The cargo runner limits git to network transports for dependency
    // fetches; this fixture's origin is a directory beside the checkout.
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
fn a_pinned_input_absent_from_its_store_is_refused_until_it_is_stored() {
    let mut deployment = Deployment::start();
    deployment.cli(&["config", "init"]);
    let archive = deployment.root.join("input.tar.gz");
    fs::write(&archive, b"quality release input\n").unwrap();
    let sha256 = hex::encode(Sha256::digest(fs::read(&archive).unwrap()));
    let uri = format!("stado://sources/{PRODUCT}/{INPUT}/{sha256}/input.tar.gz");
    let checkout = deployment.checkout(&uri, &sha256);
    let checkout = checkout.to_string_lossy().into_owned();

    let refused = deployment.run(&["quality", "check", "--root", &checkout]);
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        !refused.status.success(),
        "a check over an absent pin passed: {}",
        String::from_utf8_lossy(&refused.stdout)
    );
    assert!(
        stderr.contains(&format!("release input {INPUT} of {PRODUCT} pins {uri}"))
            && stderr.contains("is absent"),
        "the refusal does not name the input and its pin: {stderr}"
    );

    deployment.cli(&["storage", "put", &uri, &archive.to_string_lossy()]);

    let passed = deployment.cli(&["quality", "check", "--root", &checkout]);
    assert!(
        passed.contains(&format!("release input {INPUT} at {uri}"))
            && passed.contains("finds its release inputs stored"),
        "the passing check does not report the input it found: {passed}"
    );

    deployment.report["outcome"] = json!("passed");
    deployment.save();
    eprintln!("report: {}", deployment.root.join("report.json").display());
}
