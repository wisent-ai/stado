//! The full `stado quality check` runs every gate a product installation
//! declares, each kind — `format`, `lint` and `test`. The pre-push selection
//! runs format-kind gates only, which a web product may use for typecheck.
//!
//! One isolated deployment per case: a local store, committed Rust source
//! and the real `stado` executable. The declared gates run rustfmt, compile
//! test code and execute its assertion; an absent gate kind is refused.
//! The checkout commits with the identity of the account running the test,
//! and its manifest takes its schema version from Stado's own manifest.
#[path = "gate_kind/journey.rs"]
mod journey;

#[path = "command_tree/mod.rs"]
mod command_tree;

#[path = "push/mod.rs"]
mod pushed_revisions;

#[path = "gate_kind/non_release.rs"]
mod non_release;

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const PRODUCT: &str = "quality-gate-kind-test";
const MANIFEST: &str = ".wisent-release.json";

struct Deployment {
    repository: PathBuf,
    root: PathBuf,
    rustup_home: PathBuf,
    toolchain: String,
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
        eprintln!("report: {}", root.join("report.json").display());
        let report_path = root.join("report.json");
        let mut report = json!({
            "source_revision": null,
            "source_changes": null,
            "commands": [],
            "outcome": "failed",
        });
        fs::write(&report_path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
        let source_revision = git_output(
            &repository,
            &["rev-parse", "HEAD"],
            &mut report,
            &report_path,
        );
        report["source_revision"] = json!(source_revision);
        fs::write(&report_path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
        let source_changes = git_output(
            &repository,
            &["status", "--porcelain"],
            &mut report,
            &report_path,
        );
        report["source_changes"] = json!(source_changes);
        fs::write(&report_path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
        let rustup = Command::new("rustup").args(["show", "home"]).output();
        let observation = match &rustup {
            Ok(output) => json!({
                "program": "rustup",
                "args": ["show", "home"],
                "exit_status": output.status.code(),
                "stdout": String::from_utf8_lossy(&output.stdout),
                "stderr": String::from_utf8_lossy(&output.stderr),
            }),
            Err(error) => json!({
                "program": "rustup",
                "args": ["show", "home"],
                "launch_error": error.to_string(),
            }),
        };
        report["commands"].as_array_mut().unwrap().push(observation);
        fs::write(&report_path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
        let rustup = match rustup {
            Ok(output) => output,
            Err(error) => panic!(
                "rustup show home did not run: {error}; report: {}",
                report_path.display()
            ),
        };
        assert!(
            rustup.status.success(),
            "rustup show home failed: {}; report: {}",
            String::from_utf8_lossy(&rustup.stderr),
            report_path.display()
        );
        let rustup_home = PathBuf::from(String::from_utf8_lossy(&rustup.stdout).trim());
        assert!(
            !rustup_home.as_os_str().is_empty(),
            "rustup show home returned no path; report: {}",
            report_path.display()
        );
        // The exported test checkout has no rust-toolchain.toml. Pass the
        // source repository's actual active toolchain through to its gates.
        let active = Command::new("rustup")
            .args(["show", "active-toolchain"])
            .current_dir(&repository)
            .output();
        let observation = match &active {
            Ok(output) => json!({
                "program": "rustup",
                "args": ["show", "active-toolchain"],
                "directory": repository.display().to_string(),
                "exit_status": output.status.code(),
                "stdout": String::from_utf8_lossy(&output.stdout),
                "stderr": String::from_utf8_lossy(&output.stderr),
            }),
            Err(error) => json!({
                "program": "rustup",
                "args": ["show", "active-toolchain"],
                "directory": repository.display().to_string(),
                "launch_error": error.to_string(),
            }),
        };
        report["commands"].as_array_mut().unwrap().push(observation);
        fs::write(&report_path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
        let active = match active {
            Ok(output) => output,
            Err(error) => panic!(
                "rustup show active-toolchain did not run: {error}; report: {}",
                report_path.display()
            ),
        };
        assert!(
            active.status.success(),
            "rustup show active-toolchain failed: {}; report: {}",
            String::from_utf8_lossy(&active.stderr),
            report_path.display()
        );
        let active_text = String::from_utf8_lossy(&active.stdout);
        let toolchain = active_text
            .split_whitespace()
            .next()
            .expect("rustup show active-toolchain returned no selected toolchain")
            .to_string();
        Self {
            repository,
            root,
            rustup_home,
            toolchain,
            report,
        }
    }

    fn run(&mut self, args: &[&str]) -> Output {
        let output = Command::new(env!("CARGO_BIN_EXE_stado"))
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", &self.root)
            .env("RUSTUP_HOME", &self.rustup_home)
            .env("RUSTUP_TOOLCHAIN", &self.toolchain)
            .env("STADO_CONFIG", self.root.join(".stado").join("config.json"))
            .env("TMPDIR", self.root.join("tmp"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.root.join("store"))
            .stdin(Stdio::null())
            .args(args)
            .output()
            .unwrap();
        self.report["commands"].as_array_mut().unwrap().push(json!({
            "program": env!("CARGO_BIN_EXE_stado"),
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

    fn git(&mut self, directory: &Path, args: &[&str]) {
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
        self.report["commands"].as_array_mut().unwrap().push(json!({
            "directory": directory.display().to_string(),
            "program": "git",
            "args": args,
            "exit_status": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        }));
        self.save();
        assert!(
            output.status.success(),
            "git {args:?} in {}: {}",
            directory.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// A committed product checkout named `name` whose Rust `source` and
    /// quality `gates` are part of the same revision.
    fn checkout(&mut self, name: &str, source: &str, gates: Value) -> String {
        let checkout = self.root.join(name);
        fs::create_dir_all(&checkout).unwrap();
        self.git(&checkout, &["init", "--quiet", "--initial-branch=main"]);
        let own: Value =
            serde_json::from_slice(&fs::read(self.repository.join(MANIFEST)).unwrap()).unwrap();
        fs::write(checkout.join("VERSION"), "0.1.0\n").unwrap();
        fs::create_dir_all(checkout.join("src")).unwrap();
        fs::write(checkout.join("src/lib.rs"), source).unwrap();
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
        self.git(&checkout, &["add", "VERSION", "src/lib.rs", MANIFEST]);
        self.git(&checkout, &["commit", "--quiet", "-m", "declare the gates"]);
        checkout.to_string_lossy().into_owned()
    }
}

fn git_output(directory: &Path, args: &[&str], report: &mut Value, report_path: &Path) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(directory)
        .output();
    let observation = match &output {
        Ok(output) => json!({
            "program": "git",
            "args": args,
            "directory": directory.display().to_string(),
            "exit_status": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        }),
        Err(error) => json!({
            "program": "git",
            "args": args,
            "directory": directory.display().to_string(),
            "launch_error": error.to_string(),
        }),
    };
    report["commands"].as_array_mut().unwrap().push(observation);
    fs::write(report_path, serde_json::to_vec_pretty(report).unwrap()).unwrap();
    let output = match output {
        Ok(output) => output,
        Err(error) => panic!(
            "cannot run git {args:?}: {error}; report: {}",
            report_path.display()
        ),
    };
    assert!(
        output.status.success(),
        "git {args:?} failed: {}; report: {}",
        String::from_utf8_lossy(&output.stderr),
        report_path.display()
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}
