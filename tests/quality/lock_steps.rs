//! `stado quality check` asks a Cargo product's committed lock in two steps
//! whose failures mean different things: `cargo fetch` brings the sources in
//! (Git dependencies through the `git` program), and only then does
//! `cargo metadata --locked --offline` ask the committed lock. A source that
//! cannot be fetched is said as such, with no verdict on the lock; metadata's
//! own cause is preserved when it fails, including a lock that no longer
//! resolves the manifest. A current lock with a fetchable dependency passes.
//!
//! One isolated deployment: a dependency crate in its own repository with a
//! bare origin, products naming that Git dependency by `file://` origin and
//! pinned commit, and the real `stado` binary. The products are committed and
//! pushed to their own origins; a later dependency commit remains local to
//! exercise the supplementary local-pin diagnosis. Every CLI invocation has
//! its own Cargo home, so an earlier successful fetch cannot mask a failure.
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const PRODUCT: &str = "quality-lock-steps-test";
const DEPENDENCY: &str = "lockdep";
const INNER: &str = "inner";
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
        // A prior successful fetch must not mask the later unavailable origin.
        let cargo_home = self.root.join("cargo-homes").join(
            self.report["commands"]
                .as_array()
                .unwrap()
                .len()
                .to_string(),
        );
        fs::create_dir_all(&cargo_home).unwrap();
        let output = Command::new(&binary)
            .env_clear()
            .env("PATH", path)
            .env("HOME", self.root.join("home"))
            .env("STADO_CONFIG", self.root.join("home/.stado/config.json"))
            .env("CARGO_HOME", &cargo_home)
            .env("TMPDIR", self.root.join("tmp"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.root.join("store"))
            .stdin(Stdio::null())
            .args(args)
            .output()
            .unwrap();
        self.report["commands"].as_array_mut().unwrap().push(json!({
            "args": args,
            "cargo_home": cargo_home,
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

    /// A repository at `<root>/<name>` with a bare origin under
    /// `<root>/origins/<name>.git`, holding `files` (and, with `lock`, the
    /// Cargo.lock written against them), committed and pushed: its path.
    fn repository(&self, name: &str, files: &[(&str, String)], lock: bool) -> PathBuf {
        let origin = self.root.join("origins").join(format!("{name}.git"));
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
                "quality": [{"name": "fmt", "kind": "format",
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
        // The operator's global Git configuration, its hooks path included,
        // never judges the fixture's own history; the fixture's commits
        // carry their own author.
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Stado quality fixture")
        .env("GIT_AUTHOR_EMAIL", "")
        .env("GIT_COMMITTER_NAME", "Stado quality fixture")
        .env("GIT_COMMITTER_EMAIL", "")
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

#[path = "lock_steps/journeys.rs"]
mod journeys;
