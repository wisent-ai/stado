//! Drive cleanup through the real CLI, retaining receipts outside the swept home.

use serde_json::Value;
use std::fs;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub(crate) struct Fixture {
    pub(crate) evidence: PathBuf,
    pub(crate) home: PathBuf,
}

impl Fixture {
    pub(crate) fn new() -> Self {
        let evidence_root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../.wisent-output/scratch-workdirs");
        fs::create_dir_all(&evidence_root).unwrap();
        let evidence = tempfile::Builder::new()
            .prefix("run-")
            .tempdir_in(evidence_root)
            .unwrap()
            .keep();
        let home = evidence.join("home");
        fs::create_dir_all(home.join(".stado/work")).unwrap();
        let revision = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .unwrap();
        fs::write(evidence.join("revision.txt"), revision.stdout).unwrap();
        Self { evidence, home }
    }

    pub(crate) fn work(&self) -> PathBuf {
        self.home.join(".stado/work")
    }

    pub(crate) fn run(&self, step: &str, args: &[&str]) -> Output {
        let output = Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(args)
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("STADO_CONFIG", self.home.join("absent-config.json"))
            .output()
            .unwrap();
        fs::write(self.evidence.join(format!("{step}.stdout")), &output.stdout).unwrap();
        fs::write(self.evidence.join(format!("{step}.stderr")), &output.stderr).unwrap();
        fs::write(
            self.evidence.join(format!("{step}.json")),
            serde_json::to_vec_pretty(
                &serde_json::json!({"arguments": args, "exitCode": output.status.code()}),
            )
            .unwrap(),
        )
        .unwrap();
        output
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.home);
    }
}

pub(crate) fn document(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "invalid report: {error}; stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    })
}
