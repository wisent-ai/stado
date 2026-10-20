use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

pub(super) const PRODUCT: &str = "web-typecheck-test";
const MANIFEST: &str = ".wisent-release.json";

pub(super) struct Deployment {
    repository: PathBuf,
    root: PathBuf,
    report: Value,
}

impl Deployment {
    pub(super) fn start() -> Self {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let root = repository
            .join(".build/quality-web-typecheck")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(root.join("tmp")).unwrap();
        fs::create_dir_all(root.join("home")).unwrap();
        eprintln!("report: {}", root.join("report.json").display());
        let mut deployment = Self {
            repository: repository.clone(),
            root,
            report: json!({
                "source_revision": null,
                "source_changes": null,
                "commands": [],
                "outcome": "failed",
            }),
        };
        deployment.save();
        let revision = deployment.git(&repository, &["rev-parse", "HEAD"]);
        deployment.report["source_revision"] =
            json!(String::from_utf8_lossy(&revision.stdout).trim());
        deployment.save();
        let changes = deployment.git(&repository, &["status", "--porcelain"]);
        deployment.report["source_changes"] =
            json!(String::from_utf8_lossy(&changes.stdout).trim());
        deployment.save();
        deployment
    }

    pub(super) fn run(&mut self, args: &[&str]) -> Output {
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
        self.report["binary"] = json!(binary);
        self.report["commands"].as_array_mut().unwrap().push(json!({
            "program": binary,
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

    fn record_external(&mut self, directory: &Path, program: &str, args: &[&str], output: &Output) {
        self.report["commands"].as_array_mut().unwrap().push(json!({
            "directory": directory.display().to_string(),
            "program": program,
            "args": args,
            "exit_status": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        }));
        self.save();
    }

    pub(super) fn git(&mut self, directory: &Path, args: &[&str]) -> Output {
        // The fixture's origin is a directory beside the checkout; the cargo
        // runner's network-only Git transport policy does not apply to it.
        let output = Command::new("git")
            // The operator's global Git configuration, its hooks path
            // included, never judges the fixture's own history; the
            // fixture's commits carry their own author.
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "Stado quality fixture")
            .env("GIT_AUTHOR_EMAIL", "")
            .env("GIT_COMMITTER_NAME", "Stado quality fixture")
            .env("GIT_COMMITTER_EMAIL", "")
            .env_remove("GIT_ALLOW_PROTOCOL")
            .args(args)
            .current_dir(directory)
            .output()
            .unwrap();
        self.record_external(directory, "git", args, &output);
        assert!(
            output.status.success(),
            "git {args:?} in {}: {}",
            directory.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    pub(super) fn finish(&mut self, checkout: &Path) {
        let head = self.git(checkout, &["rev-parse", "HEAD"]);
        let origin = self.root.join("origin.git");
        let pushed = self.git(&origin, &["rev-parse", "refs/heads/main"]);
        let head = String::from_utf8_lossy(&head.stdout).trim().to_string();
        let pushed = String::from_utf8_lossy(&pushed.stdout).trim().to_string();
        assert_eq!(
            head, pushed,
            "the corrected source was not pushed to the fixture origin"
        );
        self.report["observed_final_state"] = json!({
            "checkout_head": head,
            "origin_main": pushed,
            "quality": "passed",
        });
        self.report["outcome"] = json!("passed");
        self.save();
        eprintln!("report: {}", self.root.join("report.json").display());
    }

    /// A web product checkout with a tsconfig.json, committed and pushed to
    /// its own origin.
    pub(super) fn checkout(&mut self) -> PathBuf {
        let origin = self.root.join("origin.git");
        let checkout = self.root.join("checkout");
        fs::create_dir_all(&origin).unwrap();
        fs::create_dir_all(&checkout).unwrap();
        self.git(
            &origin,
            &["init", "--quiet", "--bare", "--initial-branch=main"],
        );
        self.git(&checkout, &["init", "--quiet", "--initial-branch=main"]);
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
                    "quality": [{"name": "web-quality", "kind": "format", "argv": ["stado", "web", "quality"]}],
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
        fs::write(
            checkout.join("tsconfig.json"),
            "{\"compilerOptions\":{\"strict\":true}}\n",
        )
        .unwrap();
        fs::write(
            checkout.join("index.ts"),
            "export const site: string = \"typecheck\";\n",
        )
        .unwrap();
        fs::write(
            checkout.join("index.mjs"),
            "export const site = \"typecheck\";\n",
        )
        .unwrap();
        self.write_package(&checkout, None);
        self.lock_package(&checkout);
        self.git(&checkout, &["add", "."]);
        self.git(
            &checkout,
            &["commit", "--quiet", "-m", "a TypeScript web product"],
        );
        self.git(
            &checkout,
            &["remote", "add", "origin", &origin.to_string_lossy()],
        );
        self.git(&checkout, &["push", "--quiet", "origin", "main"]);
        checkout
    }

    /// Write the package scripts without changing the compiler resolution
    /// already committed in its lock.
    pub(super) fn write_package(&self, checkout: &Path, typecheck: Option<&str>) {
        let mut scripts = json!({"build": "tsc --noEmit && node index.mjs"});
        if let Some(command) = typecheck {
            scripts["typecheck"] = json!(command);
        }
        let package = json!({
            "name": PRODUCT,
            "version": "0.1.0",
            "private": true,
            "scripts": scripts,
            "devDependencies": {"typescript": "latest"},
        });
        fs::write(
            checkout.join("package.json"),
            serde_json::to_vec_pretty(&package).unwrap(),
        )
        .unwrap();
    }

    /// Resolve the fixture's real compiler once, before its initial commit.
    fn lock_package(&mut self, checkout: &Path) {
        let args = ["install", "--package-lock-only", "--ignore-scripts"];
        let output = Command::new("npm")
            .args(args)
            .env("HOME", checkout.parent().unwrap().join("home"))
            .env(
                "NPM_CONFIG_CACHE",
                checkout.parent().unwrap().join("npm-cache"),
            )
            .env("TMPDIR", checkout.parent().unwrap().join("tmp"))
            .current_dir(checkout)
            .output()
            .unwrap();
        self.record_external(checkout, "npm", &args, &output);
        assert!(
            output.status.success(),
            "npm could not write the fixture's lock: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
