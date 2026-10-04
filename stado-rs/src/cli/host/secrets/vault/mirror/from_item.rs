//! `stado credentials token register-item-local SKARBIEC ITEM FIELD -- grant issue …`:
//! the vault owner's half of `stado credentials token mint --token-item`, run
//! by the owner host's installed Stado with the vault's environment.
//!
//! It reads ITEM#FIELD with `skarbiec get`, hands that value to `skarbiec grant
//! issue` through an owner-only file that is removed again, and, when
//! `STADO_TOKEN_DESTINATION` names a path, keeps the same bearer there so the
//! consumer's file and the registered grant agree. The report is Skarbiec's,
//! without the bearer.

use std::fs;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::cli::CmdError;

const OWNER_ONLY_FILE: u32 = 0o600;
const OWNER_ONLY_DIRECTORY: u32 = 0o700;

fn invoke(skarbiec: &str, arguments: &[String]) -> Result<Value, CmdError> {
    let verb = arguments.first().map(String::as_str).unwrap_or("");
    let output = Command::new(skarbiec)
        .args(arguments)
        .output()
        .map_err(|error| {
            CmdError::click(format!("skarbiec {verb} could not start: {error}"))
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let detail = if stderr.is_empty() {
            format!("exit status {}", output.status)
        } else {
            stderr
        };
        return Err(CmdError::click(format!("skarbiec {verb} failed: {detail}"))
            .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    serde_json::from_slice(&output.stdout).map_err(|error| {
        CmdError::click(format!("skarbiec {verb} returned unreadable JSON: {error}"))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
    })
}

fn owner_only_write(path: &Path, text: &str) -> Result<(), CmdError> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(OWNER_ONLY_FILE)
        .open(path)
        .map_err(|error| {
            CmdError::click(format!("{}: {error}", path.display()))
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?;
    file.write_all(text.as_bytes()).map_err(|error| {
        CmdError::click(format!("{}: {error}", path.display()))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })
}

/// Keep the registered bearer where its consumer reads it on this host. A file
/// that already holds another bearer is refused rather than overwritten,
/// because whoever reads it would lose access silently.
fn persist(destination: &Path, token: &str) -> Result<(), CmdError> {
    if fs::symlink_metadata(destination).is_ok_and(|meta| meta.file_type().is_symlink()) {
        return Err(CmdError::refused(format!(
            "{} must not be a symlink",
            destination.display()
        )));
    }
    if destination.exists() {
        let held = fs::read_to_string(destination).map_err(|error| {
            CmdError::click(format!("{}: {error}", destination.display()))
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?;
        if held.trim() != token {
            return Err(CmdError::refused(format!(
                "{} holds another bearer; move it aside before registering this one",
                destination.display()
            )));
        }
        return Ok(());
    }
    if let Some(parent) = destination.parent() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(OWNER_ONLY_DIRECTORY)
            .create(parent)
            .map_err(|error| {
                CmdError::click(format!("{}: {error}", parent.display()))
                    .stating(crate::cli::entry::error::io_failure_code(error.kind()))
            })?;
    }
    let mut pending = destination.as_os_str().to_owned();
    pending.push(format!(".pending.{}", std::process::id()));
    let pending = PathBuf::from(pending);
    owner_only_write(&pending, token)?;
    let linked = fs::hard_link(&pending, destination);
    let _ = fs::remove_file(&pending);
    linked.map_err(|error| {
        CmdError::click(format!("{}: {error}", destination.display()))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })
}

fn register(
    skarbiec: &str,
    item: &str,
    field: &str,
    arguments: &[String],
) -> Result<Value, CmdError> {
    if arguments.get(..2) != Some(&["grant".to_string(), "issue".to_string()]) {
        return Err(CmdError::usage(
            "an existing vault field can only supply grant issue",
        ));
    }
    let source = invoke(skarbiec, &["get".to_string(), item.to_string()])?;
    let token = source
        .pointer(&format!("/fields/{field}"))
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty() && token.trim() == *token)
        .ok_or_else(|| {
            CmdError::click(format!(
                "{item}#{field} must contain one nonempty bearer without surrounding whitespace"
            ))
            .stating(crate::primitives::failure::FailureCode::Config)
        })?
        .to_string();
    let home = std::env::var("HOME").map_err(|_| {
        CmdError::click("HOME is not set").stating(crate::primitives::failure::FailureCode::Config)
    })?;
    let work = Path::new(&home).join(".stado/work/vault-token-mint");
    let scratch = work.join(format!("source-{}", std::process::id()));
    fs::DirBuilder::new()
        .recursive(true)
        .mode(OWNER_ONLY_DIRECTORY)
        .create(&scratch)
        .map_err(|error| {
            CmdError::click(format!("{}: {error}", scratch.display()))
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?;
    let bearer = scratch.join("bearer");
    let issued = owner_only_write(&bearer, &token).and_then(|()| {
        let mut issue = arguments.to_vec();
        issue.extend(["--token-file".to_string(), bearer.display().to_string()]);
        invoke(skarbiec, &issue)
    });
    let _ = fs::remove_dir_all(&scratch);
    let mut report = issued?;
    if report.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(CmdError::click(
            "skarbiec grant issue did not report a successful registration",
        )
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    if let Ok(destination) = std::env::var("STADO_TOKEN_DESTINATION") {
        if !destination.is_empty() {
            persist(Path::new(&destination), &token)?;
        }
    }
    if let Some(object) = report.as_object_mut() {
        object.remove("token");
    }
    Ok(report)
}

/// Print Skarbiec's registration report, without the bearer.
pub fn register_item_local(
    skarbiec: &str,
    item: &str,
    field: &str,
    arguments: &[String],
) -> Result<(), CmdError> {
    let report = register(skarbiec, item, field, arguments)?;
    println!("{report}");
    Ok(())
}
