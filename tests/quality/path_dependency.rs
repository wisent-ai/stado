//! `stado quality check` over a Cargo product whose manifest reaches a sibling
//! checkout with a path dependency refuses the dependency by its own name -
//! the declaring manifest, the dependency, the path and the two declarations
//! that do carry a sibling into a release - before any lock is asked about,
//! and never calls it a lockfile problem. The same product with the
//! dependency inside its own tree is not refused by that check.
//!
//! One isolated deployment: a checkout with its own origin, a sibling crate
//! beside it that the checkout's manifest names with `path = "../sibling"`,
//! and the real `stado` this test builds. The committed tree is exported
//! alone, as an install and a release build export it, so the sibling is
//! absent there; the refusal must say so and name the route, and must not
//! instruct a `Cargo.lock` rewrite.
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const PRODUCT: &str = "quality-path-dependency-test";
const SIBLING: &str = "sibling";
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
            .join(".build/quality-path-dependency")
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

    /// The sibling crate at `directory`: one library package.
    fn sibling_crate(directory: &Path) {
        fs::create_dir_all(directory.join("src")).unwrap();
        fs::write(
            directory.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{SIBLING}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
                 [dependencies]\n"
            ),
        )
        .unwrap();
        fs::write(
            directory.join("src/lib.rs"),
            "pub const SIBLING: bool = true;\n",
        )
        .unwrap();
    }

    /// A Cargo product depending on the sibling at `sibling_path` (relative
    /// to the checkout), with its lock written against it, committed and
    /// pushed to its own origin. The product's own fmt gate is the one the
    /// check would run after the lock gate.
    fn checkout(&self, name: &str, sibling_path: &str) -> PathBuf {
        let origin = self.root.join(format!("{name}-origin.git"));
        let checkout = self.root.join(name);
        fs::create_dir_all(&origin).unwrap();
        fs::create_dir_all(checkout.join("src")).unwrap();
        git(
            &origin,
            &["init", "--quiet", "--bare", "--initial-branch=main"],
        );
        git(&checkout, &["init", "--quiet", "--initial-branch=main"]);
        let own: Value =
            serde_json::from_slice(&fs::read(self.repository.join(MANIFEST)).unwrap()).unwrap();
        let manifest = json!({
            "schema_version": own["schema_version"],
            "product": PRODUCT,
            "releases": true,
            "version_source": {
                "kind": "regex",
                "path": "Cargo.toml",
                "pattern": "(?m)^version\\s*=\\s*\"(?P<version>[^\"]+)\"\\s*$"
            },
            "platforms": {
                "darwin-arm64": {
                    "runner_platform": "darwin-arm64",
                    "quality": [{
                        "name": "fmt",
                        "kind": "format",
                        "argv": ["cargo", "fmt", "--manifest-path", "Cargo.toml", "--all", "--", "--check"]
                    }],
                    "build": {"argv": ["cargo", "build", "--locked", "--release"]},
                    "stage": {"target/release/product": "product"}
                }
            },
            "promotion": {"channels": ["candidate"], "reconcile": false}
        });
        fs::write(
            checkout.join(MANIFEST),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        fs::write(
            checkout.join("Cargo.toml"),
            format!(
                "[package]\nname = \"product\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
                 [dependencies]\n{SIBLING} = {{ path = \"{sibling_path}\" }}\n"
            ),
        )
        .unwrap();
        fs::write(
            checkout.join("src/main.rs"),
            format!("fn main() {{\n    println!(\"{{}}\", {SIBLING}::SIBLING);\n}}\n"),
        )
        .unwrap();
        // The lock is written against the sibling where the manifest names
        // it, so the lock itself is current: only the tree is incomplete.
        let generated = Command::new("cargo")
            .args(["generate-lockfile", "--offline"])
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
        git(&checkout, &["add", "."]);
        git(
            &checkout,
            &[
                "commit",
                "--quiet",
                "-m",
                "a Cargo product with a path dependency",
            ],
        );
        git(
            &checkout,
            &["remote", "add", "origin", &origin.to_string_lossy()],
        );
        git(&checkout, &["push", "--quiet", "origin", "main"]);
        checkout
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

const OUTSIDE_SENTENCE: &str = "which is outside the committed tree of";
const ROUTE_SENTENCE: &str = "Declare it as a git dependency on its canonical origin";
const LOCK_INSTRUCTION: &str = "Cargo.lock";

#[test]
fn a_path_dependency_outside_the_committed_tree_is_refused_by_name_not_as_a_stale_lock() {
    let mut deployment = Deployment::start();
    deployment.cli(&["config", "init"]);

    // Outside: the sibling beside the checkout, reached with `../sibling`.
    Deployment::sibling_crate(&deployment.root.join(SIBLING));
    let outside = deployment.checkout("outside", &format!("../{SIBLING}"));
    let refused = deployment.run(&["quality", "check", "--root", &outside.to_string_lossy()]);
    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(
        !refused.status.success(),
        "a tree whose dependency is outside it passed: {said}"
    );
    assert!(
        said.contains(OUTSIDE_SENTENCE)
            && said.contains(&format!("declares dependency {SIBLING} at path"))
            && said.contains("(package product)")
            && said.contains(ROUTE_SENTENCE),
        "the refusal names the manifest, the dependency, the path and the route: {said}"
    );
    assert!(
        !said.contains("stale") && !said.contains("cargo update"),
        "the refusal must not read as a lockfile problem: {said}"
    );
    assert!(
        said.contains(&format!("no {LOCK_INSTRUCTION} change creates it")),
        "the refusal says a lock rewrite cannot create the source: {said}"
    );
    deployment.report["outside_refusal"] = json!(said.trim());

    // Inside: the same sibling vendored into the checkout, reached with
    // `./sibling`; the lock gate passes it to the product's own gates.
    let inside = deployment.root.join("inside");
    fs::create_dir_all(&inside).unwrap();
    Deployment::sibling_crate(&inside.join(SIBLING));
    let inside = deployment.checkout("inside", &format!("./{SIBLING}"));
    let checked = deployment.run(&["quality", "check", "--root", &inside.to_string_lossy()]);
    let said = String::from_utf8_lossy(&checked.stderr);
    assert!(
        !said.contains(OUTSIDE_SENTENCE),
        "a dependency inside the tree was refused as outside it: {said}"
    );
    assert!(
        checked.status.success(),
        "the vendored product's check did not pass: {said}{}",
        String::from_utf8_lossy(&checked.stdout)
    );
    deployment.report["outcome"] = json!("passed");
    deployment.save();
    eprintln!("report: {}", deployment.root.join("report.json").display());
}
