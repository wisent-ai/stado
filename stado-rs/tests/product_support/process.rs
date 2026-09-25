use super::Run;
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::{
    fs::{self, File},
    io::BufReader,
    path::PathBuf,
    process::{Command, ExitStatus},
};

pub struct Observation {
    pub status: ExitStatus,
    pub directory: PathBuf,
}

impl Observation {
    pub fn passed(&self) -> Result<()> {
        ensure!(
            self.status.success(),
            "command failed with {}; evidence: {}; stderr: {}",
            self.status,
            self.directory.display(),
            fs::read_to_string(self.directory.join("stderr.log"))?
        );
        Ok(())
    }

    pub fn refused(&self) -> Result<()> {
        ensure!(
            !self.status.success(),
            "command unexpectedly succeeded; evidence: {}",
            self.directory.display()
        );
        Ok(())
    }

    pub fn json(&self) -> Result<Value> {
        self.passed()?;
        serde_json::from_reader(BufReader::new(File::open(
            self.directory.join("stdout.log"),
        )?))
        .with_context(|| {
            format!(
                "decode actual command output in {}",
                self.directory.display()
            )
        })
    }
}

pub fn command(run: &mut Run, mut command: Command) -> Result<Observation> {
    let directory = run
        .evidence
        .join(format!("command-{}", run.observations.len() + 1));
    fs::create_dir(&directory)?;
    let mut record = json!({"program": command.get_program().to_string_lossy(),
        "arguments": command.get_args().map(|s| s.to_string_lossy()).collect::<Vec<_>>(),
        "cwd": command.get_current_dir(),
        "environment_keys": command.get_envs().map(|(name, _)| name.to_string_lossy()).collect::<Vec<_>>(),
        "started_at": chrono::Utc::now().to_rfc3339(), "evidence": directory});
    fs::write(
        directory.join("command.json"),
        serde_json::to_vec_pretty(&record)?,
    )?;
    command
        .stdout(File::create(directory.join("stdout.log"))?)
        .stderr(File::create(directory.join("stderr.log"))?);
    let outcome = command.status();
    record["completed_at"] = json!(chrono::Utc::now().to_rfc3339());
    record["exit_code"] = json!(outcome.as_ref().ok().and_then(ExitStatus::code));
    record["process_status"] = json!(outcome.as_ref().ok().map(ToString::to_string));
    record["spawn_error"] = json!(outcome.as_ref().err().map(ToString::to_string));
    fs::write(
        directory.join("command.json"),
        serde_json::to_vec_pretty(&record)?,
    )?;
    run.observations.push(record);
    run.record("running", None)?;
    Ok(Observation {
        status: outcome
            .with_context(|| format!("starting command; evidence: {}", directory.display()))?,
        directory,
    })
}
