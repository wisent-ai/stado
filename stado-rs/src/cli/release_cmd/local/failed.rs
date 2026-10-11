//! A Stado release that failed on this host, and the way back from it.
//!
//! An installed Stado whose own rules refuse the configuration the release
//! before it served (a component stopping with a configuration failure)
//! cannot serve on this host however often its supervisor restarts it: a
//! release crash-looped for a day, taking every adapter of the workstation
//! with it, and nothing stopped it or put the previous release back. So the
//! failing release records itself as failed on this host, beside its
//! retained archive, and reinstalls the release that ran before it (the one
//! whose retained copy is byte for byte `~/.stado/bin/stado.previous`) through
//! the same checked install as `release local restore --previous`. A release
//! recorded as failed is never installed here again, by a delivery or a
//! restore, until the record is removed; so two releases that both refuse
//! the configuration do not restore each other in turn, and product sync
//! does not reinstall the release that just failed. `stado release local
//! list` reads every retained release with what it is to this host: the
//! installed one, the rollback target, and any that failed with its record.

use std::path::PathBuf;

use clap::Args;
use serde_json::{json, Value};

use crate::cli::CmdError;

/// The record beside a retained release that says it failed on this host.
const FAILED_RECORD: &str = "failed-on-this-host.json";

/// Where the failure record of `version` lives on this host.
fn record(version: &str) -> Result<PathBuf, CmdError> {
    let platform = crate::self_update::platform_triple_short()
        .map_err(|error| CmdError::declaration(error.to_string()))?;
    Ok(crate::config_file::expand_tilde("~")
        .join(".stado")
        .join("releases")
        .join("stado")
        .join(version)
        .join(platform)
        .join(FAILED_RECORD))
}

/// Refuse a release this host recorded as failed, naming the record and why
/// it failed.
pub(super) fn refuse_failed(version: &str) -> Result<(), CmdError> {
    let path = record(version)?;
    match std::fs::read_to_string(&path) {
        Ok(text) => Err(CmdError::refused(format!(
            "Stado {version} failed on this host and is not installed here again: {}; the record is {} \
             — remove it once the cause is repaired",
            text.trim(),
            path.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(CmdError::click(format!(
            "cannot read whether Stado {version} failed on this host from {}: {error}",
            path.display()
        ))
        .stating(crate::cli::entry::error::io_failure_code(error.kind()))),
    }
}

/// Record the running release as failed on this host for `cause`, then
/// reinstall the release that ran before it. The answer says what was done.
pub(crate) async fn roll_back_failed_release(cause: &str) -> Result<String, CmdError> {
    let version = env!("CARGO_PKG_VERSION");
    let path = record(version)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            CmdError::click(format!("cannot prepare {}: {error}", parent.display()))
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?;
    }
    let written = serde_json::json!({
        "version": version,
        "pid": std::process::id(),
        "at": chrono::Utc::now().to_rfc3339(),
        "cause": cause,
    });
    std::fs::write(&path, format!("{written}\n")).map_err(|error| {
        CmdError::click(format!(
            "cannot record Stado {version} as failed on this host at {}: {error}",
            path.display()
        ))
        .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    let restored = super::restore::restore_previous().await?;
    Ok(format!(
        "Stado {version} refused this host's configuration ({cause}); recorded as failed at {} and \
         reinstalled Stado {restored}, the release that ran before it",
        path.display()
    ))
}

#[derive(Args)]
pub struct ReleaseListLocalArgs {
    /// Print the releases as one JSON document.
    #[arg(long)]
    json: bool,
}

fn unreadable(path: &std::path::Path, error: std::io::Error) -> CmdError {
    CmdError::click(format!("release local list: {} cannot be read: {error}", path.display()))
        .stating(crate::cli::entry::error::io_failure_code(error.kind()))
}

/// The file's bytes, or nothing when it is absent; any other failure names it.
fn read_if_present(path: &std::path::Path) -> Result<Option<Vec<u8>>, CmdError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(unreadable(path, error)),
    }
}

/// One retained release and what it is to this host.
struct Retained {
    version: String,
    installed: bool,
    rollback_target: bool,
    failed: Option<Value>,
}

/// Every Stado release this host retains, with what it is to the host.
pub(in crate::cli::release_cmd) async fn list_local(args: &ReleaseListLocalArgs) -> Result<(), CmdError> {
    let platform = crate::self_update::platform_triple_short()
        .map_err(|error| CmdError::declaration(error.to_string()))?;
    let stado = crate::config_file::expand_tilde("~").join(".stado");
    let retained = stado.join("releases").join("stado");
    let installed = read_if_present(&stado.join("bin").join("stado.release-version"))?
        .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_string());
    let previous = read_if_present(&stado.join("bin").join("stado.previous"))?;
    let entries = match std::fs::read_dir(&retained) {
        Ok(entries) => entries.collect::<Result<Vec<_>, _>>().map_err(|error| unreadable(&retained, error))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(unreadable(&retained, error)),
    };
    let mut releases: Vec<Retained> = Vec::new();
    for entry in entries {
        let version = entry.file_name().to_string_lossy().into_owned();
        let directory = entry.path().join(platform);
        if !directory.is_dir() {
            continue;
        }
        let staged = read_if_present(&directory.join("stado"))?;
        let record = directory.join(FAILED_RECORD);
        let failed = match read_if_present(&record)? {
            Some(bytes) => Some(serde_json::from_slice::<Value>(&bytes).map_err(|error| {
                CmdError::click(format!("release local list: {} is not a failure record: {error}", record.display()))
            })?),
            None => None,
        };
        releases.push(Retained {
            installed: installed.as_deref() == Some(version.as_str()),
            rollback_target: previous.is_some() && staged == previous,
            failed,
            version,
        });
    }
    releases.sort_by(|left, right| left.version.cmp(&right.version));
    if args.json {
        let listed: Vec<Value> = releases
            .iter()
            .map(|release| {
                json!({ "version": release.version, "installed": release.installed,
                    "rollbackTarget": release.rollback_target, "failed": release.failed })
            })
            .collect();
        println!("{}", json!({ "platform": platform, "releases": listed }));
        return Ok(());
    }
    if releases.is_empty() {
        println!("this host retains no Stado release under {}", retained.display());
    }
    for release in &releases {
        let mut roles = Vec::new();
        if release.installed {
            roles.push("installed".to_string());
        }
        if release.rollback_target {
            roles.push("rollback target".to_string());
        }
        if let Some(record) = &release.failed {
            roles.push(format!("failed on this host: {record}"));
        }
        println!("{}  {}", release.version, roles.join("; "));
    }
    Ok(())
}
