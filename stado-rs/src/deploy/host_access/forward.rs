//! Service-directory forward markers.
//!
//! The marker is a cache of one directory endpoint, never a second route map.
//! Skarbiec and the other credential consumers already read the one-line file
//! at `$HOME/.stado/forwards/<service>.local`, so every writer here preserves
//! that exact path and content shape.

use std::path::PathBuf;

use serde::Serialize;

use crate::deploy::{host_channel, production_runner, DeployError};
use crate::targets::ComputeTarget;

#[derive(Debug, Clone, Serialize)]
pub struct ForwardMarker {
    pub service: String,
    pub url: String,
    pub marker: String,
    pub location: &'static str,
}

fn valid_service(service: &str) -> bool {
    !service.is_empty()
        && !service.starts_with('.')
        && service
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn checked_service(service: &str) -> Result<(), DeployError> {
    if valid_service(service) {
        Ok(())
    } else {
        Err(DeployError(format!(
            "service {service:?} cannot name a forward marker; use its service_directory.services key"
        ))
        .stating(crate::primitives::failure::FailureCode::Refused))
    }
}

/// `~/.stado/forwards` under the home every Stado file is kept under
/// ([`crate::config_file::home`]): the same directory the resolver's
/// publication writes, which reads it the same way.
fn forwards_dir() -> Result<PathBuf, DeployError> {
    let home = crate::config_file::home().map_err(|cause| {
        DeployError(format!(
            "{cause}; set HOME to the account that owns .stado/forwards"
        ))
        .stating(crate::primitives::failure::FailureCode::Config)
    })?;
    Ok(home.join(".stado").join("forwards"))
}

pub fn local_path(service: &str) -> Result<PathBuf, DeployError> {
    checked_service(service)?;
    Ok(forwards_dir()?.join(format!("{service}.local")))
}

/// Read a local marker only when it is a regular file. Following a symlink
/// would turn a route listing into an arbitrary file read.
pub fn read_local(service: &str) -> Result<Option<ForwardMarker>, DeployError> {
    let marker = local_path(service)?;
    match marker.symlink_metadata() {
        Ok(metadata) if metadata.file_type().is_file() => {}
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(DeployError::from(error)),
    }
    let read = std::fs::read_to_string(&marker).map_err(|error| {
        DeployError(format!("could not read {}: {error}", marker.display()))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    // The marker's first line is the address; a marker without one names
    // nothing to dial, and reading it as an empty address sent every
    // consumer to "" instead of saying which file is wrong.
    let Some(url) = read
        .lines()
        .next()
        .map(str::trim)
        .filter(|url| !url.is_empty())
    else {
        return Err(DeployError(format!(
            "forward marker {} names no address on its first line; `stado service directory \
             publish` writes it again",
            marker.display()
        ))
        .stating(crate::primitives::failure::FailureCode::Config));
    };
    let url = url.to_string();
    Ok(Some(ForwardMarker {
        service: service.to_string(),
        url,
        marker: marker.display().to_string(),
        location: "local",
    }))
}

/// Atomically write the exact one-line marker shape the credential bridge
/// consumes, owner-readable and owner-writable only.
pub fn open_local(service: &str, url: &str) -> Result<ForwardMarker, DeployError> {
    use std::os::unix::fs::PermissionsExt;

    let marker = local_path(service)?;
    let directory = marker.parent().ok_or_else(|| {
        DeployError("forward marker has no parent directory".to_string())
            .stating(crate::primitives::failure::FailureCode::Config)
    })?;
    std::fs::create_dir_all(directory).map_err(DeployError::from)?;
    std::fs::set_permissions(
        directory,
        std::fs::Permissions::from_mode(crate::primitives::file_mode::owner_only_directory()),
    )
    .map_err(DeployError::from)?;
    // The marker is written whole and durably, owner-only, through the one
    // writer Stado replaces its files with; a private copy renamed it into
    // place without flushing the file or its directory.
    crate::primitives::document::persist(&marker, format!("{url}\n").as_bytes())
        .map_err(DeployError::from)?;
    Ok(ForwardMarker {
        service: service.to_string(),
        url: url.to_string(),
        marker: marker.display().to_string(),
        location: "local",
    })
}

pub fn close_local(service: &str) -> Result<bool, DeployError> {
    let marker = local_path(service)?;
    match std::fs::remove_file(&marker) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(DeployError::io(format!(
            "could not remove {}",
            marker.display()
        ))(error)),
    }
}

pub async fn open_remote(
    target: &ComputeTarget,
    service: &str,
    url: &str,
) -> Result<ForwardMarker, DeployError> {
    checked_service(service)?;
    let runner = production_runner();
    let home = host_channel::remote_home(target, &runner).await?;
    let path = format!("{home}/.stado/forwards/{service}.local");
    let quoted_path = crate::deploy::shlex_quote(&path);
    let quoted_url = crate::deploy::shlex_quote(url);
    let script = format!(
        "set -eu\npath={quoted_path}\ndirectory=\"${{path%/*}}\"\n/bin/mkdir -p \"$directory\"\n/bin/chmod 700 \"$directory\"\numask 077\nstaging=$(/usr/bin/mktemp \"$path.staging.XXXXXX\")\ntrap '/bin/rm -f \"$staging\"' EXIT HUP INT TERM\nprintf '%s\\n' {quoted_url} > \"$staging\"\n/bin/chmod 600 \"$staging\"\n/bin/mv \"$staging\" \"$path\"\n"
    );
    let output = host_channel::run_script(target, &script, &runner).await?;
    if !output.succeeded() {
        return Err(DeployError::unreachable(host_channel::last_error_line(
            &output,
            "remote forward marker write failed",
        )));
    }
    Ok(ForwardMarker {
        service: service.to_string(),
        url: url.to_string(),
        marker: path,
        location: "remote",
    })
}

pub async fn close_remote(target: &ComputeTarget, service: &str) -> Result<bool, DeployError> {
    checked_service(service)?;
    let runner = production_runner();
    let home = host_channel::remote_home(target, &runner).await?;
    let path = format!("{home}/.stado/forwards/{service}.local");
    let quoted_path = crate::deploy::shlex_quote(&path);
    let script = format!(
        "set -eu\npath={quoted_path}\nif [ -f \"$path\" ]; then /bin/rm -f \"$path\"; echo STADO_ROUTE_REMOVED; else echo STADO_ROUTE_ABSENT; fi\n"
    );
    let output = host_channel::run_script(target, &script, &runner).await?;
    if !output.succeeded() {
        return Err(DeployError::unreachable(host_channel::last_error_line(
            &output,
            "remote forward marker removal failed",
        )));
    }
    Ok(output
        .stdout
        .lines()
        .any(|line| line.trim() == "STADO_ROUTE_REMOVED"))
}
