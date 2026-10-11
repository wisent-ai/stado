//! `stado quality check` asks a Cargo product's committed lock in two steps
//! whose failures mean different things: `cargo fetch` brings the sources in
//! (Git dependencies through the `git` program), and only then does
//! `cargo metadata --locked --offline` ask the committed lock. A source that
//! cannot be fetched is said as such, with no verdict on the lock; a lock
//! that does not resolve its manifest is refused as such; a current lock
//! with a fetchable Git dependency passes.
//!
//! One isolated deployment: a dependency crate in its own repository with a
//! bare origin, products whose manifests name it as a Git dependency by its
//! `file://` origin and pinned commit, each committed and pushed to its own
//! origin, and the real `stado` this test builds, run with an isolated home
//! so Cargo's Git cache starts empty.
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const PRODUCT: &str = "quality-lock-steps-test";
const DEPENDENCY: &str = "lockdep";
const INNER: &str = "inner";
const MANIFEST: &str = ".wisent-release.json";
const FETCH_FAILED: &str = "so whether its Cargo.lock resolves was not asked";
const LOCK_REFUSED: &str = "the committed Cargo.lock does not resolve its manifest";

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
            .join(".build/quality-lock-steps")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(root.join("tmp")).unwrap();
        fs::create_dir_all(root.join("home")).unwrap();
        fs::create_dir_all(root.join("store")).unwrap();
        let source_revision = git_output(&repository, &["rev-parse", "HEAD"]);
        Self {
            repository,
            root,
            report: json!({"source_revision": source_revision, "commands": [], "outcome": "failed"}),
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

    fn save(&self) {
        let report = serde_json::to_vec_pretty(&self.report).unwrap();
        fs::write(self.root.join("report.json"), report).unwrap();
    }

    /// A repository at `<root>/<name>` with a bare origin beside it, holding
    /// `files` (and, with `lock`, the Cargo.lock written against them)
    /// committed and pushed: the repository's path.
    fn repository(&self, name: &str, files: &[(&str, String)], lock: bool) -> PathBuf {
        let origin = self.root.join(format!("{name}-origin.git"));
        let checkout = self.root.join(name);
        fs::create_dir_all(checkout.join("src")).unwrap();
        fs::create_dir_all(&origin).unwrap();
        git(
            &origin,
            &["init", "--quiet", "--bare", "--initial-branch=main"],
        );
        git(&checkout, &["init", "--quiet", "--initial-branch=main"]);
        for (path, text) in files {
            fs::write(checkout.join(path), text).unwrap();
        }
        if lock {
            let generated = Command::new("cargo")
                .arg("generate-lockfile")
                .env("CARGO_NET_GIT_FETCH_WITH_CLI", "true")
                .current_dir(&checkout)
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .output()
                .unwrap();
            assert!(
                generated.status.success(),
                "cargo could not write the fixture's lock: {}",
                String::from_utf8_lossy(&generated.stderr)
            );
        }
        git(&checkout, &["add", "."]);
        git(&checkout, &["commit", "--quiet", "-m", "fixture"]);
        git(
            &checkout,
            &["remote", "add", "origin", &origin.to_string_lossy()],
        );
        git(&checkout, &["push", "--quiet", "origin", "main"]);
        checkout
    }

    /// A Cargo product naming the dependency by `url` at `commit`, with a
    /// lock written against it, committed and pushed to its own origin.
    fn checkout(&self, name: &str, url: &str, commit: &str) -> PathBuf {
        let own: Value =
            serde_json::from_slice(&fs::read(self.repository.join(MANIFEST)).unwrap()).unwrap();
        let release = json!({
            "schema_version": own["schema_version"], "product": PRODUCT, "releases": true,
            "version_source": {"kind": "regex", "path": "Cargo.toml",
                "pattern": "(?m)^version\\s*=\\s*\"(?P<version>[^\"]+)\"\\s*$"},
            "platforms": {"darwin-arm64": {"runner_platform": "darwin-arm64",
                "quality": [{"name": "fmt",
                    "argv": ["cargo", "fmt", "--manifest-path", "Cargo.toml", "--all", "--", "--check"]}],
                "build": {"argv": ["cargo", "build", "--locked", "--release"]},
                "stage": {"target/release/product": "product"}}},
            "promotion": {"channels": ["candidate"], "reconcile": false}
        });
        let files = [
            (MANIFEST, serde_json::to_string_pretty(&release).unwrap()),
            (
                "Cargo.toml",
                format!(
                    "[package]\nname = \"product\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
                     [dependencies]\n{DEPENDENCY} = {{ git = \"{url}\", rev = \"{commit}\" }}\n"
                ),
            ),
            (
                "src/main.rs",
                format!("fn main() {{\n    println!(\"{{}}\", {DEPENDENCY}::FETCHED);\n}}\n"),
            ),
        ];
        self.repository(name, &files, true)
    }

    /// Add a path dependency inside the checkout to its manifest without
    /// touching the committed lock, commit and push: the lock no longer
    /// resolves the manifest.
    fn outdate_lock(checkout: &Path) {
        let inner = checkout.join(INNER);
        fs::create_dir_all(inner.join("src")).unwrap();
        fs::write(
            inner.join("Cargo.toml"),
            format!("[package]\nname = \"{INNER}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        )
        .unwrap();
        fs::write(inner.join("src/lib.rs"), "pub const INNER: bool = true;\n").unwrap();
        let manifest = checkout.join("Cargo.toml");
        let mut text = fs::read_to_string(&manifest).unwrap();
        text.push_str(&format!("{INNER} = {{ path = \"./{INNER}\" }}\n"));
        fs::write(&manifest, text).unwrap();
        git(checkout, &["add", "."]);
        git(
            checkout,
            &[
                "commit",
                "--quiet",
                "-m",
                "a dependency the lock does not name",
            ],
        );
        git(checkout, &["push", "--quiet", "origin", "main"]);
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
fn a_fetch_failure_and_an_unresolved_lock_are_told_apart_and_a_current_lock_passes() {
    let mut deployment = Deployment::start();
    let initialized = deployment.run(&["config", "init"]);
    let said = String::from_utf8_lossy(&initialized.stderr).into_owned();
    assert!(initialized.status.success(), "stado config init: {said}");
    let crate_manifest =
        format!("[package]\nname = \"{DEPENDENCY}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n");
    let dependency = deployment.repository(
        DEPENDENCY,
        &[
            ("Cargo.toml", crate_manifest),
            (
                "src/lib.rs",
                "pub const FETCHED: bool = true;\n".to_string(),
            ),
        ],
        false,
    );
    let commit = git_output(&dependency, &["rev-parse", "HEAD"]);
    let origin = deployment.root.join(format!("{DEPENDENCY}-origin.git"));
    let url = format!("file://{}", origin.display());

    // Current: the Git dependency is fetched and the committed lock resolves.
    let current = deployment.checkout("current", &url, &commit);
    let checked = deployment.run(&["quality", "check", "--root", &current.to_string_lossy()]);
    let said = String::from_utf8_lossy(&checked.stderr).into_owned();
    assert!(
        checked.status.success(),
        "a current lock did not pass: {said}"
    );
    deployment.report["current"] = json!(said.trim());

    // Stale: the manifest names a dependency the committed lock lacks.
    let stale = deployment.checkout("stale", &url, &commit);
    Deployment::outdate_lock(&stale);
    let refused = deployment.run(&["quality", "check", "--root", &stale.to_string_lossy()]);
    let said = String::from_utf8_lossy(&refused.stderr).into_owned();
    assert!(!refused.status.success(), "a stale lock passed: {said}");
    assert!(
        said.contains(LOCK_REFUSED) && said.contains("--locked --offline"),
        "the stale lock is refused as a lock that does not resolve: {said}"
    );
    assert!(
        !said.contains(FETCH_FAILED),
        "a stale lock is not a fetch failure: {said}"
    );
    deployment.report["stale"] = json!(said.trim());

    // Unfetchable: the dependency's origin is gone after the lock was written.
    let unfetchable = deployment.checkout("unfetchable", &url, &commit);
    fs::rename(&origin, deployment.root.join("moved-origin.git")).unwrap();
    let failed = deployment.run(&["quality", "check", "--root", &unfetchable.to_string_lossy()]);
    let said = String::from_utf8_lossy(&failed.stderr).into_owned();
    assert!(
        !failed.status.success(),
        "an unfetchable source passed: {said}"
    );
    assert!(
        said.contains("cargo fetch --manifest-path Cargo.toml failed")
            && said.contains(FETCH_FAILED)
            && said.contains(DEPENDENCY),
        "the fetch failure names the step, the dependency and that no lock verdict exists: {said}"
    );
    assert!(
        !said.contains(LOCK_REFUSED),
        "a fetch failure is not a lock refusal: {said}"
    );
    deployment.report["unfetchable"] = json!(said.trim());

    deployment.report["outcome"] = json!("passed");
    deployment.save();
    eprintln!("report: {}", deployment.root.join("report.json").display());
}
