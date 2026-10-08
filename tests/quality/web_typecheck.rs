//! `stado quality check` over a web product refuses a TypeScript project that
//! declares no typecheck script, because its build checks types and the gate
//! would otherwise pass a commit the build refuses; the same product passes
//! once it declares one.
//!
//! One isolated checkout with its own origin and a web platform whose gate is
//! `stado web quality`, run by the real `stado` this test builds (its
//! directory leads PATH, so the gate's `stado` is the same binary). The
//! project has a tsconfig.json and a build script, and npm writes its lock.
//! Without a typecheck script the check must refuse naming the script to
//! declare; after a commit that declares one, the gate installs the locked
//! tree, runs it and passes.
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const PRODUCT: &str = "web-typecheck-test";
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
            .join(".build/quality-web-typecheck")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(root.join("tmp")).unwrap();
        fs::create_dir_all(root.join("home")).unwrap();
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

    /// A web product checkout with a tsconfig.json, committed and pushed to
    /// its own origin.
    fn checkout(&self) -> PathBuf {
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
        let manifest = json!({
            "schema_version": own["schema_version"],
            "product": PRODUCT,
            "releases": true,
            "version_source": {"kind": "json", "path": "package.json", "pointer": "/version"},
            "platforms": {
                "web": {
                    "runner_platform": "darwin-arm64",
                    "quality": [{"name": "web-quality", "argv": ["stado", "web", "quality"]}],
                    "build": {"argv": ["stado", "web", "build"]},
                    "stage": {"dist/site.tar.gz": "site.tar.gz"}
                }
            },
            "promotion": {"channels": ["candidate"], "reconcile": false}
        });
        fs::write(
            checkout.join(MANIFEST),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        fs::write(checkout.join("tsconfig.json"), "{}\n").unwrap();
        fs::write(
            checkout.join("index.mjs"),
            "export const site = \"typecheck\";\n",
        )
        .unwrap();
        write_package(&checkout, None);
        git(&checkout, &["add", "."]);
        git(
            &checkout,
            &["commit", "--quiet", "-m", "a TypeScript web product"],
        );
        git(
            &checkout,
            &["remote", "add", "origin", &origin.to_string_lossy()],
        );
        git(&checkout, &["push", "--quiet", "origin", "main"]);
        checkout
    }
}

/// package.json, with `typecheck` declared when given, and the lock npm
/// writes for it.
fn write_package(checkout: &Path, typecheck: Option<&str>) {
    let mut scripts = json!({"build": "node index.mjs"});
    if let Some(command) = typecheck {
        scripts["typecheck"] = json!(command);
    }
    let package = json!({
        "name": PRODUCT,
        "version": "0.1.0",
        "private": true,
        "scripts": scripts,
    });
    fs::write(
        checkout.join("package.json"),
        serde_json::to_vec_pretty(&package).unwrap(),
    )
    .unwrap();
    let status = Command::new("npm")
        .args(["install", "--package-lock-only", "--ignore-scripts"])
        .current_dir(checkout)
        .stdout(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "npm could not write the fixture's lock");
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
fn a_typescript_web_product_without_a_typecheck_script_is_refused_until_it_declares_one() {
    let mut deployment = Deployment::start();
    let checkout = deployment.checkout();
    let root = checkout.to_string_lossy().into_owned();

    let refused = deployment.run(&["quality", "check", "--root", &root]);
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&refused.stdout),
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(
        !refused.status.success(),
        "a TypeScript product without typecheck passed: {said}"
    );
    assert!(
        said.contains("declares no typecheck script")
            && said.contains("\"typecheck\": \"tsc --noEmit\""),
        "the refusal does not name the script to declare: {said}"
    );

    write_package(&checkout, Some("node --check index.mjs"));
    git(&checkout, &["add", "package.json", "package-lock.json"]);
    git(&checkout, &["commit", "--quiet", "-m", "declare typecheck"]);
    git(&checkout, &["push", "--quiet", "origin", "main"]);

    let passed = deployment.run(&["quality", "check", "--root", &root]);
    let said = String::from_utf8_lossy(&passed.stdout).into_owned();
    assert!(
        passed.status.success(),
        "the product declaring typecheck was refused: {said}{}",
        String::from_utf8_lossy(&passed.stderr)
    );
    assert!(
        said.contains(&format!("stado web quality: {PRODUCT} passed typecheck")),
        "the gate did not run the declared typecheck: {said}"
    );

    deployment.report["outcome"] = json!("passed");
    deployment.save();
    eprintln!("report: {}", deployment.root.join("report.json").display());
}
