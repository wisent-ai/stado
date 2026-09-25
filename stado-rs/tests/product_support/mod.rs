//! One isolated `stado product` journey: a copy of the compiled `stado`, a
//! copy of the authoritative catalog, an empty consumer home under
//! `target/product-runs/`, and every command's argv, output and exit status
//! retained under the evidence directory with the executed build's identity.

mod process;
pub mod releases;
use anyhow::{Context, Result};
pub use process::{command, Observation};
use serde_json::{json, Value};
use std::{fs, path::PathBuf};

pub struct Run {
    pub root: PathBuf,
    pub evidence: PathBuf,
    pub home: PathBuf,
    pub catalog: PathBuf,
    pub workspace: PathBuf,
    pub binary: PathBuf,
    pub observations: Vec<Value>,
    executable_sha256: Option<String>,
}

/// The build these tests were compiled with, as `stado --version` prints it.
const IDENTITY: &str = concat!(
    "stado ",
    env!("CARGO_PKG_VERSION"),
    " (rev ",
    env!("STADO_SOURCE_REVISION"),
    ")"
);

impl Run {
    pub fn new(area: &str) -> Result<Self> {
        let package = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let id = uuid::Uuid::new_v4().to_string();
        let root = package.join("target/product-runs").join(area).join(&id);
        let output = std::env::var_os("WISENT_TEST_EVIDENCE_DIR")
            .or_else(|| std::env::var_os("WISENT_OUTPUT_DIR"))
            .map(PathBuf::from)
            .unwrap_or_else(|| package.join("target/product-evidence"));
        let evidence = output.join(format!("product-{area}")).join(id);
        let home = root.join("home");
        fs::create_dir_all(&home)?;
        fs::create_dir_all(&evidence)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
            fs::set_permissions(&evidence, fs::Permissions::from_mode(0o700))?;
        }
        let catalog = root.join("products.yml");
        let workspace = stado_product::common::Runtime::new(None)?.workspace;
        let binary = root.join("stado");
        let mut run = Self {
            root,
            evidence,
            home,
            catalog,
            workspace,
            binary,
            observations: Vec::new(),
            executable_sha256: None,
        };
        run.record("running", None)?;
        let prepared = (|| -> Result<()> {
            fs::copy(package.join("../catalog/products.yml"), &run.catalog)?;
            fs::copy(env!("CARGO_BIN_EXE_stado"), &run.binary)?;
            run.executable_sha256 = Some(stado_product::common::sha256(&run.binary)?);
            let mut cmd = std::process::Command::new(&run.binary);
            cmd.arg("--version");
            let identity = command(&mut run, cmd)?;
            identity.passed()?;
            let reported = fs::read_to_string(identity.directory.join("stdout.log"))?;
            anyhow::ensure!(reported.trim() == IDENTITY,
                "the executed Stado does not report this test's identity: expected {IDENTITY:?}, observed {reported:?}");
            Ok(())
        })();
        if let Err(error) = prepared {
            return run.finish(Err(error));
        }
        Ok(run)
    }

    /// `stado product --catalog <this run's catalog> ARGUMENTS` in the
    /// isolated consumer home.
    pub fn product(&self, arguments: &[&str]) -> std::process::Command {
        let mut command = std::process::Command::new(&self.binary);
        command
            .arg("product")
            .arg("--catalog")
            .arg(&self.catalog)
            .args(arguments)
            .env("HOME", &self.home)
            .env("WISENT_WORKSPACE", &self.workspace)
            .env("WISENT_OUTPUT_DIR", self.evidence.join("product"))
            .env("TMPDIR", &self.root)
            .current_dir(env!("CARGO_MANIFEST_DIR"));
        command
    }

    pub fn authority(&self) -> Result<Value> {
        serde_yaml::from_slice(&fs::read(&self.catalog)?).context("read actual persisted authority")
    }

    pub fn finish<T>(self, mut result: Result<T>) -> Result<T> {
        let retained = self
            .catalog
            .try_exists()
            .and_then(|exists| {
                if exists {
                    fs::copy(&self.catalog, self.evidence.join("final-authority.yml")).map(|_| ())
                } else {
                    Ok(())
                }
            })
            .context("retain final product test authority");
        let cleanup = fs::remove_dir_all(&self.root).context("remove isolated product test state");
        for outcome in [retained, cleanup] {
            if let Err(failure) = outcome {
                result = Err(match result {
                    Err(error) => {
                        error.context(format!("test finalization also failed: {failure:#}"))
                    }
                    Ok(_) => failure,
                });
            }
        }
        let recorded = match &result {
            Err(error) => self.record("failed", Some(format!("{error:#}"))),
            Ok(_) => self.record("passed", None),
        };
        if let Err(failure) = recorded {
            return Err(match result {
                Err(error) => error.context(format!(
                    "recording the test result also failed: {failure:#}"
                )),
                Ok(_) => failure,
            });
        }
        result
    }

    fn record(&self, status: &str, error: Option<String>) -> Result<()> {
        let value = json!({"source_revision": env!("STADO_SOURCE_REVISION"), "binary": self.binary,
            "executable_sha256": self.executable_sha256,
            "status": status, "error": error, "state_directory": self.root, "evidence": self.evidence,
            "recorded_at": chrono::Utc::now().to_rfc3339(), "observations": self.observations});
        fs::write(
            self.evidence.join("report.json"),
            serde_json::to_vec_pretty(&value)?,
        )?;
        Ok(())
    }
}
