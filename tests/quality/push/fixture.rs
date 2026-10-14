use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub struct Fixture {
    pub repository: PathBuf,
    pub root: PathBuf,
    pub checkout: PathBuf,
    pub remote: PathBuf,
    report: Value,
}
impl Fixture {
    pub fn new() -> Self {
        let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("Stado repository")
            .to_path_buf();
        let root = repository
            .join(".build/quality-push")
            .join(uuid::Uuid::new_v4().to_string());
        let checkout = root.join("home/product");
        fs::create_dir_all(&checkout).expect("create isolated checkout");
        let mut this = Self {
            remote: root.join("remote.git"),
            repository,
            root,
            checkout,
            report: json!({"verdict": "failed", "commands": []}),
        };
        let revision = this.success(
            Command::new("git")
                .current_dir(&this.repository)
                .arg("rev-parse")
                .arg("HEAD"),
        );
        this.report["source_revision"] = json!(String::from_utf8_lossy(&revision.stdout).trim());
        let diff = this.success(
            Command::new("git")
                .current_dir(&this.repository)
                .arg("diff")
                .arg("HEAD"),
        );
        this.report["source_diff"] = json!(String::from_utf8_lossy(&diff.stdout));
        let digest = this.success(Command::new("shasum").arg(env!("CARGO_BIN_EXE_stado")));
        this.report["binary_digest"] = json!(String::from_utf8_lossy(&digest.stdout).trim());
        this.save();
        this
    }
    pub fn run(&mut self, command: &mut Command) -> Output {
        let invocation = format!("{command:?}");
        let output = command.output().expect("launch real test command");
        self.report["commands"]
            .as_array_mut()
            .expect("command evidence")
            .push(json!({
                "command": invocation, "exit_status": output.status.code(),
                "stdout": String::from_utf8_lossy(&output.stdout),
                "stderr": String::from_utf8_lossy(&output.stderr)
            }));
        self.save();
        output
    }
    pub fn success(&mut self, command: &mut Command) -> Output {
        let output = self.run(command);
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }
    pub fn git(&self) -> Command {
        let mut command = Command::new("git");
        command.current_dir(&self.checkout);
        self.environment(&mut command);
        command
    }
    pub fn stado(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command.current_dir(&self.checkout);
        self.environment(&mut command);
        command
    }
    fn environment(&self, command: &mut Command) {
        let home = self.root.join("home");
        command
            .env_clear()
            .env(
                "PATH",
                std::env::var_os("PATH").expect("test toolchain PATH"),
            )
            .env("HOME", &home)
            .env("STADO_CONFIG", home.join(".stado/config.json"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", home.join("store"))
            .env("STADO_BIN", env!("CARGO_BIN_EXE_stado"))
            .env("STADO_TEST_CHECKOUT", &self.checkout);
        if let Some(rustup) = std::env::var_os("RUSTUP_HOME") {
            command.env("RUSTUP_HOME", rustup);
        } else if let Some(home) = std::env::var_os("HOME") {
            command.env("RUSTUP_HOME", PathBuf::from(home).join(".rustup"));
        }
    }
    pub fn commit(&mut self) -> String {
        self.success(
            self.git()
                .arg("add")
                .arg("source.rs")
                .arg("VERSION")
                .arg(".wisent-release.json"),
        );
        self.success(
            self.git()
                .arg("commit")
                .arg("--quiet")
                .arg("-m")
                .arg("Declare product source"),
        );
        let output = self.success(self.git().arg("rev-parse").arg("HEAD"));
        String::from_utf8(output.stdout)
            .expect("commit identity")
            .trim()
            .to_owned()
    }
    pub fn published(&mut self) -> String {
        let output = self.success(
            Command::new("git")
                .arg("--git-dir")
                .arg(&self.remote)
                .arg("rev-parse")
                .arg("refs/heads/delivery"),
        );
        String::from_utf8(output.stdout)
            .expect("remote identity")
            .trim()
            .to_owned()
    }
    pub fn passed(&mut self) {
        self.report["verdict"] = json!("passed");
        self.save();
    }
    fn save(&self) {
        fs::write(
            self.root.join("report.json"),
            serde_json::to_vec_pretty(&self.report).expect("encode real journey evidence"),
        )
        .expect("retain real journey evidence");
    }
}
