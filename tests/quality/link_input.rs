//! `stado quality check` over a web product whose gate links a release input
//! beside the tree (`stado web quality --link-input`) passes every time it is
//! run, and leaves nothing of a run behind in the checkout.
//!
//! The link lands in the tree's parent, the work area a release job owns.
//! When the check exported the tree straight under `.wisent-output/quality/`,
//! that parent was shared by every run: the first run's link outlived it and
//! every later check of the product refused with "already exists, and a link
//! would replace it", so no commit of it could be handed off.
//!
//! One isolated deployment: a local store holding the input, a checkout with
//! its own origin, and the real `stado` this test builds (its directory leads
//! PATH, so the gate's `stado` is the same binary). The product's package.json
//! depends on `file:../sibling`, which the gate answers by linking the pinned
//! archive's `sibling` directory there. The check runs twice and both runs
//! must pass, the second after the first left its work area; the checkout's
//! `.wisent-output/quality` must be empty after each.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const PRODUCT: &str = "quality-link-input-test";
const INPUT: &str = "sibling";
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
            .join(".build/quality-link-input")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(root.join("tmp")).unwrap();
        fs::create_dir_all(root.join("home")).unwrap();
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
        let binary = PathBuf::from(env!("CARGO_BIN_EXE_stado"));
        let inherited = std::env::var_os("PATH").unwrap();
        let path = std::env::join_paths(
            std::iter::once(binary.parent().unwrap().to_path_buf())
                .chain(std::env::split_paths(&inherited)),
        )
        .unwrap();
        let output = Command::new(&binary)
            .env_clear()
            .env("PATH", path)
            .env("HOME", self.root.join("home"))
            .env("STADO_CONFIG", self.root.join("home/.stado/config.json"))
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
            "stado {args:?}: {}{}",
            String::from_utf8_lossy(&output.stdout),
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

    /// The sibling package as an archive whose `sibling` directory the gate
    /// links: answers the archive's path.
    fn sibling_archive(&self) -> PathBuf {
        let staging = self.root.join("archive");
        let package = staging.join(INPUT);
        fs::create_dir_all(&package).unwrap();
        fs::write(
            package.join("package.json"),
            serde_json::to_vec_pretty(&json!({"name": INPUT, "version": "0.1.0"})).unwrap(),
        )
        .unwrap();
        fs::write(package.join("index.mjs"), "export const sibling = true;\n").unwrap();
        let archive = self.root.join("sibling.tar.gz");
        let status = Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(&staging)
            .arg(INPUT)
            .status()
            .unwrap();
        assert!(status.success(), "tar could not write the input archive");
        archive
    }

    /// A web product depending on `file:../sibling`, pinning the archive at
    /// `uri`, committed and pushed to its own origin. npm writes its lock
    /// against the sibling placed where the link will be.
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
        let link = format!("../{INPUT}={INPUT}/{INPUT}");
        let manifest = json!({
            "schema_version": own["schema_version"],
            "product": PRODUCT,
            "releases": true,
            "version_source": {"kind": "json", "path": "package.json", "pointer": "/version"},
            "platforms": {
                "web": {
                    "runner_platform": "darwin-arm64",
                    "quality": [{
                        "name": "web-quality",
                        "argv": ["stado", "web", "quality", "--link-input", link]
                    }],
                    "build": {"argv": ["stado", "web", "build", "--link-input", link]},
                    "stage": {"dist/site.tar.gz": "site.tar.gz"}
                }
            },
            "promotion": {"channels": ["candidate"], "reconcile": false},
            "inputs": {
                INPUT: {"uri": uri, "sha256": sha256, "mount": INPUT, "extract": true}
            }
        });
        fs::write(
            checkout.join(MANIFEST),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        fs::write(
            checkout.join("index.mjs"),
            "import { sibling } from \"sibling/index.mjs\";\nconsole.log(sibling);\n",
        )
        .unwrap();
        let package = json!({
            "name": PRODUCT,
            "version": "0.1.0",
            "private": true,
            "scripts": {"build": "node index.mjs"},
            "dependencies": {INPUT: format!("file:../{INPUT}")},
        });
        fs::write(
            checkout.join("package.json"),
            serde_json::to_vec_pretty(&package).unwrap(),
        )
        .unwrap();
        let sibling = self.root.join(INPUT);
        fs::create_dir_all(&sibling).unwrap();
        fs::copy(
            self.root.join("archive").join(INPUT).join("package.json"),
            sibling.join("package.json"),
        )
        .unwrap();
        let status = Command::new("npm")
            .args(["install", "--package-lock-only", "--ignore-scripts"])
            .current_dir(&checkout)
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "npm could not write the fixture's lock");
        fs::remove_dir_all(&sibling).unwrap();
        git(&checkout, &["add", "."]);
        git(
            &checkout,
            &["commit", "--quiet", "-m", "a web product linking an input"],
        );
        git(
            &checkout,
            &["remote", "add", "origin", &origin.to_string_lossy()],
        );
        git(&checkout, &["push", "--quiet", "origin", "main"]);
        checkout
    }

    /// One `stado quality check` of `checkout`, which must link the input,
    /// pass, and leave the checkout's quality scratch directory empty.
    fn check_passes_and_cleans_up(&mut self, checkout: &Path, which: &str) {
        let root = checkout.to_string_lossy().into_owned();
        let passed = self.cli(&["quality", "check", "--root", &root]);
        assert!(
            passed.contains("stado web: linked")
                && passed.contains("finds its release inputs stored"),
            "the {which} check did not link the input and pass: {passed}"
        );
        let left = left_behind(checkout);
        assert!(
            left.is_empty(),
            "the {which} check left {left:?} in .wisent-output/quality"
        );
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

/// What a check left in the checkout's quality scratch directory.
fn left_behind(checkout: &Path) -> Vec<String> {
    match fs::read_dir(checkout.join(".wisent-output/quality")) {
        Ok(entries) => entries
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => panic!("cannot read the quality scratch directory: {error}"),
    }
}

#[test]
fn a_linked_input_is_linked_again_on_every_check_and_nothing_is_left_behind() {
    let mut deployment = Deployment::start();
    deployment.cli(&["config", "init"]);
    let archive = deployment.sibling_archive();
    let sha256 = hex::encode(Sha256::digest(fs::read(&archive).unwrap()));
    let uri = format!("stado://sources/{PRODUCT}/{INPUT}/{sha256}/sibling.tar.gz");
    deployment.cli(&["storage", "put", &uri, &archive.to_string_lossy()]);
    let checkout = deployment.checkout(&uri, &sha256);

    deployment.check_passes_and_cleans_up(&checkout, "first");
    deployment.check_passes_and_cleans_up(&checkout, "repeated");

    deployment.report["outcome"] = json!("passed");
    deployment.save();
    eprintln!("report: {}", deployment.root.join("report.json").display());
}
