use serde_json::{json, Value};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

pub type Result<T, E = Box<dyn std::error::Error>> = std::result::Result<T, E>;

pub struct Record {
    pub root: PathBuf,
    pub directory: PathBuf,
    pub identity: String,
    value: Value,
}

pub fn required(name: &str) -> Result<String> {
    let value = env::var(name)
        .map_err(|cause| format!("{name} must name an isolated qualification input: {cause}"))?;
    if value.trim().is_empty() {
        return Err(format!("{name} must not be empty").into());
    }
    Ok(value)
}

pub fn require(condition: bool, message: impl Into<String>) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(message.into().into())
    }
}

impl Record {
    pub fn new() -> Result<Self> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()?;
        let instant = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let identity = format!("stado_notification_{}_{}", std::process::id(), instant);
        let base = root.join("build/database-notifications");
        fs::create_dir_all(&base)?;
        let directory = base.join(&identity);
        fs::create_dir(&directory)?;
        let record = Self {
            root,
            directory,
            identity,
            value: json!({
                "started_unix_nanos": instant.to_string(), "commands": [], "observations": [], "verdict": "failed"
            }),
        };
        record.persist()?;
        Ok(record)
    }

    fn persist(&self) -> Result<()> {
        fs::write(
            self.directory.join("report.json"),
            serde_json::to_vec_pretty(&self.value)?,
        )?;
        Ok(())
    }

    pub fn observe(&mut self, operation: &str, state: Value) -> Result<()> {
        self.value["observations"]
            .as_array_mut()
            .ok_or("invalid observation register")?
            .push(json!({"operation": operation, "state": state}));
        self.persist()
    }

    fn command(&mut self, program: &str, args: &[&str]) -> Result<String> {
        let output = Command::new(program)
            .args(args)
            .current_dir(&self.root)
            .output();
        match output {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                self.value["commands"].as_array_mut().ok_or("invalid command register")?
                    .push(json!({"program": program, "args": args, "exit_status": output.status.code(), "stdout": stdout, "stderr": stderr}));
                self.persist()?;
                require(
                    output.status.success(),
                    format!("{program} {args:?} failed: {}: {stderr}", output.status),
                )?;
                Ok(stdout.trim().to_owned())
            }
            Err(cause) => {
                self.value["commands"]
                    .as_array_mut()
                    .ok_or("invalid command register")?
                    .push(
                        json!({"program": program, "args": args, "start_error": cause.to_string()}),
                    );
                self.persist()?;
                Err(cause.into())
            }
        }
    }

    pub fn provenance(&mut self) -> Result<()> {
        let revision = self.command("git", &["rev-parse", "HEAD"])?;
        self.command(
            "git",
            &[
                "diff",
                "--exit-code",
                "HEAD",
                "--",
                "stado-rs",
                "tests/database/notifications",
            ],
        )?;
        let untracked = self.command(
            "git",
            &[
                "ls-files",
                "--others",
                "--exclude-standard",
                "--",
                "stado-rs",
                "tests/database/notifications",
            ],
        )?;
        require(
            untracked.is_empty(),
            format!("qualification source has untracked files: {untracked}"),
        )?;
        let binary = env::current_exe()?;
        let digest = self.command(
            "shasum",
            &[
                "-a",
                "256",
                binary.to_str().ok_or("non-UTF-8 test executable path")?,
            ],
        )?;
        let digest = digest
            .split_whitespace()
            .next()
            .ok_or("shasum returned no executable digest")?;
        let manifest_path = required("STADO_TEST_BUILD_MANIFEST")?;
        let manifest: Value = serde_json::from_slice(&fs::read(&manifest_path)?)?;
        require(
            manifest["source_revision"].as_str() == Some(revision.as_str()),
            "build manifest source_revision does not match the current source revision",
        )?;
        require(
            manifest["binary_sha256"].as_str() == Some(digest),
            "build manifest binary_sha256 does not match this notification test executable",
        )?;
        self.value["source_revision"] = json!(revision);
        self.value["binary"] = json!({"path": binary, "sha256": digest});
        self.value["build_manifest"] = manifest;
        self.persist()
    }

    pub fn finish(&mut self, result: &Result<()>) -> Result<()> {
        self.value["verdict"] = json!(if result.is_ok() { "passed" } else { "failed" });
        if let Err(cause) = result {
            self.value["error"] = json!(cause.to_string());
        }
        self.value["finished_unix_nanos"] = json!(SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_nanos()
            .to_string());
        self.persist()
    }
}
