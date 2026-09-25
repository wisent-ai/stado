use super::{atomic_json, now, sha256, Runtime};
use anyhow::{bail, Context, Result};
use serde_json::json;
use std::{
    fs::{self, File},
    path::PathBuf,
    process::{Command, Output, Stdio},
    sync::LazyLock,
};

static EXECUTABLE_HASH: LazyLock<std::result::Result<String, String>> = LazyLock::new(|| {
    std::env::current_exe()
        .context("locating the running Stado executable")
        .and_then(|path| sha256(&path))
        .map_err(|error| format!("{error:#}"))
});

fn recorded(command: &mut Command) -> Result<(Output, PathBuf)> {
    let runtime = Runtime::new(None)?;
    let folder = runtime
        .output
        .join("commands")
        .join(uuid::Uuid::new_v4().to_string());
    fs::create_dir_all(&folder)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o700))?;
    }
    let stdout = folder.join("stdout.log");
    let stderr = folder.join("stderr.log");
    command
        .stdout(Stdio::from(File::create(&stdout)?))
        .stderr(Stdio::from(File::create(&stderr)?));
    let mut report = json!({
        "started_at": now(), "program": command.get_program().to_string_lossy(),
        "args": command.get_args().map(|s| s.to_string_lossy().into_owned()).collect::<Vec<_>>(),
        "cwd": command.get_current_dir().map(|p| p.to_string_lossy()),
        "source_revision": crate::build().source_revision,
        "executable_sha256": EXECUTABLE_HASH.as_ref().ok(), "state": "starting"
    });
    let record = folder.join("command.json");
    atomic_json(&record, &report)?;
    if let Err(error) = &*EXECUTABLE_HASH {
        report["state"] = json!("failed");
        report["error"] = json!(error);
        report["finished_at"] = json!(now());
        atomic_json(&record, &report)?;
        bail!(
            "cannot attest the running Stado executable: {error}; evidence {}",
            folder.display()
        );
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            report["state"] = json!("failed");
            report["error"] = json!(error.to_string());
            report["finished_at"] = json!(now());
            atomic_json(&record, &report)?;
            return Err(error).with_context(|| {
                format!(
                    "starting {:?}; evidence {}",
                    command.get_program(),
                    folder.display()
                )
            });
        }
    };
    report["pid"] = json!(child.id());
    report["state"] = json!("running");
    let running_record = atomic_json(&record, &report);
    let status = match child.wait() {
        Ok(status) => status,
        Err(error) => {
            report["state"] = json!("failed");
            report["error"] = json!(error.to_string());
            report["finished_at"] = json!(now());
            atomic_json(&record, &report)?;
            return Err(error).with_context(|| {
                format!(
                    "waiting for {:?}; evidence {}",
                    command.get_program(),
                    folder.display()
                )
            });
        }
    };
    report["state"] = json!(if status.success() {
        "succeeded"
    } else {
        "failed"
    });
    report["exit_status"] = json!(status.code());
    report["process_status"] = json!(status.to_string());
    report["finished_at"] = json!(now());
    if let Err(error) = &running_record {
        report["recording_error"] = json!(format!("{error:#}"));
    }
    atomic_json(&record, &report)?;
    running_record.with_context(|| {
        format!(
            "recording running command {:?}; observed {status}; evidence {}",
            command.get_program(),
            folder.display()
        )
    })?;
    let output = Output {
        status,
        stdout: fs::read(&stdout).with_context(|| format!("reading {}", stdout.display()))?,
        stderr: fs::read(&stderr).with_context(|| format!("reading {}", stderr.display()))?,
    };
    Ok((output, folder))
}

pub fn capture(command: &mut Command) -> Result<Output> {
    recorded(command).map(|(output, _)| output)
}

pub fn checked(command: &mut Command) -> Result<Output> {
    let (output, evidence) = recorded(command)?;
    if !output.status.success() {
        bail!(
            "{:?} failed ({}); evidence {}: {}{}",
            command.get_program(),
            output.status,
            evidence.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(output)
}
