//! What exactly is on the host under one path, and whether anything
//! executable still calls it.

use super::*;

pub(super) async fn remote_file_identity(
    target: &crate::targets::ComputeTarget,
    path: &str,
    runner: &crate::deploy::Runner,
) -> Result<Value, CmdError> {
    for words in [
        vec!["/bin/test", "-f", path],
        vec!["/bin/test", "!", "-L", path],
    ] {
        let shape = host_channel::run_program(target, &words, runner)
            .await
            .map_err(click)?;
        if !shape.ok() {
            return Err(CmdError::click(format!(
                "{}: obsolete release-control artifact {path} must be a regular non-symlink file",
                target.name
            )));
        }
    }
    let digest = host_channel::run_program(target, &["/usr/bin/shasum", "-a", "256", path], runner)
        .await
        .map_err(click)?;
    if !digest.ok() {
        return Err(CmdError::click(format!(
            "{}: cannot hash obsolete release-control artifact {path}: {}",
            target.name,
            host_channel::last_error_line(&digest, "shasum failed")
        )));
    }
    let sha256 = digest
        .stdout
        .split_whitespace()
        .next()
        .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or_else(|| {
            CmdError::click(format!(
                "{}: shasum returned no SHA-256 for {path}",
                target.name
            ))
        })?;
    let metadata =
        host_channel::run_program(target, &["/usr/bin/stat", "-f", "%z %Lp", path], runner)
            .await
            .map_err(click)?;
    if !metadata.ok() {
        return Err(CmdError::click(format!(
            "{}: cannot inspect obsolete release-control artifact {path}: {}",
            target.name,
            host_channel::last_error_line(&metadata, "stat failed")
        )));
    }
    let mut fields = metadata.stdout.split_ascii_whitespace();
    let size = fields
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| {
            CmdError::click(format!(
                "{}: stat returned no byte size for {path}",
                target.name
            ))
        })?;
    let mode = fields
        .next()
        .and_then(|value| u32::from_str_radix(value, 8).ok())
        .filter(|mode| *mode <= 0o7777)
        .map(|mode| format!("{mode:04o}"))
        .ok_or_else(|| {
            CmdError::click(format!(
                "{}: stat returned no four-digit mode for {path}",
                target.name
            ))
        })?;
    let transaction = format!(
        "{}-{}",
        chrono::Utc::now().format("%Y%m%dT%H%M%SZ"),
        uuid::Uuid::new_v4().simple()
    );
    Ok(json!({
        "path": path,
        "transaction": transaction,
        "sha256": sha256,
        "size": size,
        "mode": mode,
    }))
}
pub(super) async fn require_no_executable_caller(
    target: &crate::targets::ComputeTarget,
    path: &str,
    runner: &crate::deploy::Runner,
) -> Result<(), CmdError> {
    let processes =
        host_channel::run_program(target, &["/bin/ps", "-axo", "pid=,command="], runner)
            .await
            .map_err(click)?;
    if !processes.ok() {
        return Err(CmdError::click(format!(
            "{}: cannot prove obsolete executable {path} has no caller: {}",
            target.name,
            host_channel::last_error_line(&processes, "ps failed")
        )));
    }
    if let Some(caller) = processes.stdout.lines().find(|line| {
        line.split_ascii_whitespace()
            .skip(1)
            .any(|argument| argument == path)
    }) {
        return Err(CmdError::click(format!(
            "{}: obsolete executable {path} is still referenced by process {}",
            target.name,
            caller.trim()
        )));
    }
    Ok(())
}
