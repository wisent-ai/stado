mod version;

use anyhow::{Context, Result};
use serde_json::{json, Value};
use stado_product::{cargo_lock, common, export_committed_source};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

struct Journey {
    root: PathBuf,
    report: Value,
}

impl Journey {
    fn run(&mut self, directory: &Path, program: &str, args: &[&str]) -> Result<String> {
        let output = Command::new(program)
            .args(args)
            .current_dir(directory)
            .output()?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        self.report["commands"]
            .as_array_mut()
            .context("commands array")?
            .push(json!({
                "program": program, "args": args, "cwd": directory,
                "status": output.status.code(), "stdout": stdout, "stderr": stderr
            }));
        anyhow::ensure!(output.status.success(), "{program} {args:?}: {stderr}");
        Ok(stdout.trim().to_owned())
    }

    fn commit(&mut self, checkout: &Path) -> Result<String> {
        self.run(checkout, "git", &["add", "."])?;
        self.run(
            checkout,
            "git",
            &[
                "-c",
                "user.name=Stado test",
                "-c",
                "user.email=stado-test@wisent.invalid",
                "commit",
                "--quiet",
                "-m",
                "Committed build input",
            ],
        )?;
        self.run(checkout, "git", &["rev-parse", "HEAD"])
    }

    fn build(&mut self, tree: &Path, revision: &str) -> Result<()> {
        let mut command = Command::new(std::env::var("CARGO")?);
        command
            .args(["build", "--locked", "--offline"])
            .current_dir(tree)
            .env("CARGO_TARGET_DIR", tree.join("target"))
            .env("CARGO_BUILD_BUILD_DIR", tree.join("target"));
        match common::checked(&mut command) {
            Ok(output) => {
                self.report["commands"]
                    .as_array_mut()
                    .context("commands array")?
                    .push(json!({
                        "program": "cargo", "args": ["build", "--locked", "--offline"],
                        "cwd": tree, "revision": revision, "status": output.status.code(),
                        "stdout": String::from_utf8_lossy(&output.stdout),
                        "stderr": String::from_utf8_lossy(&output.stderr)
                    }));
                Ok(())
            }
            Err(error) => {
                self.report["commands"]
                    .as_array_mut()
                    .context("commands array")?
                    .push(json!({
                        "program": "cargo", "args": ["build", "--locked", "--offline"],
                        "cwd": tree, "revision": revision, "failure": format!("{error:#}")
                    }));
                Err(cargo_lock::explain(error, tree, "lock-journey", revision))
            }
        }
    }

    fn exercise(&mut self, repository: &Path) -> Result<()> {
        // The quality gate runs this journey in an exported tree with no Git
        // checkout and states the commit it exported (`WISENT_SOURCE_COMMIT`):
        // that tree is the commit, with nothing uncommitted. Run from the
        // checkout, its own HEAD and working-tree diff are recorded.
        match std::env::var("WISENT_SOURCE_COMMIT") {
            Ok(commit) => {
                self.report["source_revision"] = json!(commit);
                self.report["source_diff"] = json!("");
            }
            Err(std::env::VarError::NotPresent) => {
                self.report["source_revision"] =
                    json!(self.run(repository, "git", &["rev-parse", "HEAD"])?);
                self.report["source_diff"] =
                    json!(self.run(repository, "git", &["diff", "--binary", "HEAD"])?);
            }
            Err(error) => anyhow::bail!("WISENT_SOURCE_COMMIT is set but cannot be read: {error}"),
        }
        let checkout = self.root.join("source");
        fs::create_dir_all(checkout.join("src"))?;
        self.run(
            &checkout,
            "git",
            &["init", "--quiet", "--initial-branch=main"],
        )?;
        let package: toml::Value =
            toml::from_str(include_str!("../../../stado-rs/product/Cargo.toml"))?;
        let version = package["package"]["version"]
            .as_str()
            .context("package version")?;
        let edition = package["package"]["edition"]
            .as_str()
            .context("package edition")?;
        let manifest = format!("[package]\nname = \"lock-journey\"\nversion = \"{version}\"\nedition = \"{edition}\"\n\n[dependencies]\n");
        fs::write(checkout.join("Cargo.toml"), &manifest)?;
        self.report["test_binary_sha256"] = json!(common::sha256(&std::env::current_exe()?)?);
        fs::write(checkout.join("src/main.rs"), "fn main() {}\n")?;
        fs::write(checkout.join(".gitignore"), "/target\n")?;
        let cargo = std::env::var("CARGO")?;
        self.run(&checkout, &cargo, &["generate-lockfile", "--offline"])?;
        self.commit(&checkout)?;
        let dependency = checkout.join("dependency");
        fs::create_dir_all(dependency.join("src"))?;
        fs::write(dependency.join("Cargo.toml"), format!("[package]\nname = \"lock-dependency\"\nversion = \"{version}\"\nedition = \"{edition}\"\n"))?;
        fs::write(
            dependency.join("src/lib.rs"),
            "pub fn message() -> &'static str { \"locked dependency executed\" }\n",
        )?;
        fs::write(
            checkout.join("Cargo.toml"),
            format!("{manifest}lock-dependency = {{ path = \"dependency\" }}\n"),
        )?;
        fs::write(
            checkout.join("src/main.rs"),
            "fn main() { println!(\"{}\", lock_dependency::message()); }\n",
        )?;
        let broken = self.commit(&checkout)?;
        let tree = self.root.join("broken-export");
        export_committed_source(&checkout, &broken, &tree)?;
        let lock = fs::read(tree.join("Cargo.lock"))?;
        let failure = self
            .build(&tree, &broken)
            .expect_err("unlocked committed dependency must fail");
        let source = failure
            .downcast_ref::<cargo_lock::UnlockedManifest>()
            .with_context(|| {
                format!("failure was not attributed to committed source: {failure:#}")
            })?;
        assert_eq!(source.revision, broken);
        assert_eq!(source.manifest, Path::new("Cargo.toml"));
        assert!(source
            .dependencies
            .iter()
            .any(|name| name.contains("lock-dependency")));
        let said = failure.to_string();
        assert!(
            said.contains(&broken) && said.contains("Cargo.toml") && said.contains("Cargo.lock")
        );
        assert!(said.contains("commit Cargo.lock") && said.contains("lock-dependency"));
        assert_eq!(
            fs::read(tree.join("Cargo.lock"))?,
            lock,
            "diagnosis must not repair the export"
        );
        self.report["source_refusal"] = json!(said);
        self.run(&checkout, &cargo, &["generate-lockfile", "--offline"])?;
        let repaired = self.commit(&checkout)?;
        let current = self.root.join("current-export");
        export_committed_source(&checkout, &repaired, &current)?;
        self.build(&current, &repaired)?;
        let binary = current.join("target/debug/lock-journey");
        assert_eq!(
            self.run(&current, binary.to_str().context("binary path")?, &[])?,
            "locked dependency executed"
        );
        self.changed_requirement(&checkout, &manifest, version, edition)?;
        fs::write(
            checkout.join("src/main.rs"),
            "fn main() { missing_function(); }\n",
        )?;
        let invalid = self.commit(&checkout)?;
        let unrelated = self.root.join("compiler-export");
        export_committed_source(&checkout, &invalid, &unrelated)?;
        let other = self
            .build(&unrelated, &invalid)
            .expect_err("invalid source must fail compilation");
        assert!(
            other
                .downcast_ref::<cargo_lock::UnlockedManifest>()
                .is_none(),
            "a current lock must not be blamed for a compiler error"
        );
        self.report["compiler_refusal"] = json!(format!("{other:#}"));
        Ok(())
    }
}

fn main() -> Result<()> {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?;
    let root = repository
        .join(".build/product-lock")
        .join(uuid::Uuid::new_v4().to_string());
    fs::create_dir_all(root.join("home"))?;
    // Required explicitly: the isolated product home must not hide the real toolchain.
    std::env::var("CARGO_HOME").context("CARGO_HOME is required")?;
    std::env::var("RUSTUP_HOME").context("RUSTUP_HOME is required")?;
    std::env::set_var("HOME", root.join("home"));
    std::env::set_var("WISENT_OUTPUT_DIR", root.join("commands"));
    let mut journey = Journey {
        root: root.clone(),
        report: json!({"result": "failed", "commands": []}),
    };
    let result = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        journey.exercise(&repository)
    })) {
        Ok(result) => result,
        Err(panic) => {
            let detail = if let Some(message) = panic.downcast_ref::<String>() {
                message.clone()
            } else if let Some(message) = panic.downcast_ref::<&str>() {
                message.to_string()
            } else {
                "assertion panicked; see test stderr".to_owned()
            };
            Err(anyhow::anyhow!(detail))
        }
    };
    match &result {
        Ok(()) => journey.report["result"] = json!("passed"),
        Err(error) => journey.report["error"] = json!(format!("{error:#}")),
    }
    fs::write(
        root.join("report.json"),
        serde_json::to_vec_pretty(&journey.report)?,
    )?;
    eprintln!("report: {}", root.join("report.json").display());
    result
}
