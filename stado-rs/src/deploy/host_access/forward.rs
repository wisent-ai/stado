//! Service-directory forward markers.
//!
//! The marker is a cache of one directory endpoint, never a second route map.
//! Skarbiec and the other credential consumers already read the one-line file
//! at `$HOME/.stado/forwards/<service>.local`, so every writer here preserves
//! that exact path and content shape.

use std::{
    io::Write,
    path::{Path, PathBuf},
};

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
        )))
    }
}

fn forwards_dir() -> Result<PathBuf, DeployError> {
    let home = std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .ok_or_else(|| {
            DeployError(
                "HOME is not set; set it to the account that owns .stado/forwards".to_string(),
            )
        })?;
    Ok(Path::new(&home).join(".stado").join("forwards"))
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
        Err(error) => return Err(DeployError(error.to_string())),
    }
    let url = std::fs::read_to_string(&marker)
        .map_err(|error| DeployError(format!("could not read {}: {error}", marker.display())))?
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
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
    let directory = marker
        .parent()
        .ok_or_else(|| DeployError("forward marker has no parent directory".to_string()))?;
    std::fs::create_dir_all(directory).map_err(|error| DeployError(error.to_string()))?;
    std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| DeployError(error.to_string()))?;
    let mut staging = tempfile::NamedTempFile::new_in(directory)
        .map_err(|error| DeployError(error.to_string()))?;
    staging
        .write_all(url.as_bytes())
        .and_then(|()| staging.write_all(b"\n"))
        .map_err(|error| DeployError(error.to_string()))?;
    staging
        .as_file()
        .set_permissions(std::fs::Permissions::from_mode(0o600))
        .map_err(|error| DeployError(error.to_string()))?;
    staging
        .persist(&marker)
        .map_err(|error| DeployError(error.error.to_string()))?;
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
        Err(error) => Err(DeployError(format!(
            "could not remove {}: {error}",
            marker.display()
        ))),
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
    if !output.ok() {
        return Err(DeployError(host_channel::last_error_line(
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
    if !output.ok() {
        return Err(DeployError(host_channel::last_error_line(
            &output,
            "remote forward marker removal failed",
        )));
    }
    Ok(output
        .stdout
        .lines()
        .any(|line| line.trim() == "STADO_ROUTE_REMOVED"))
}
