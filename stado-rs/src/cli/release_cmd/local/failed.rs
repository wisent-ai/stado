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
//! does not reinstall the release that just failed.

use std::path::PathBuf;

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
