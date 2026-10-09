//! The web edge as a role of the host's one Stado process.
//!
//! One service per repository: the edge is Stado's function, so it is not a
//! unit of its own. `stado serve --edge-caddy <program> --edge-caddyfile
//! <path>` runs the reverse proxy under `com.wisent.stado`, and `stado web
//! edge` delivers the generated Caddyfile to that path.
//!
//! Caddy is kept for what it does alone: ordering and renewing a certificate
//! for every hostname in the file. It runs with `--watch`, so a delivered
//! Caddyfile is loaded without restarting anything. Its exit is this role's
//! failure, and the supervisor ends the whole process on it, so the init
//! system restarts `com.wisent.stado` rather than keeping a host that
//! silently stopped terminating TLS.

use std::path::PathBuf;

use crate::cli::CmdError;

/// What a host declared as the edge serves before its first delivery: no
/// site at all. `stado web edge hostnames` compares against this and delivers
/// the generated file over it; Caddy's watch loads that file when it lands.
const UNDELIVERED: &str = "# No hostname has been delivered to this edge yet. `stado web route` \
                           and `stado web edge hostnames` replace this file.\n";

pub(crate) async fn run(caddy: PathBuf, caddyfile: PathBuf) -> Result<(), CmdError> {
    if !caddyfile.is_file() {
        if let Some(directory) = caddyfile.parent() {
            std::fs::create_dir_all(directory).map_err(|error| {
                CmdError::click(format!(
                    "the edge configuration directory {} could not be created: {error}",
                    directory.display()
                ))
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
            })?;
        }
        std::fs::write(&caddyfile, UNDELIVERED).map_err(|error| {
            CmdError::click(format!(
                "the edge configuration {} could not be written: {error}",
                caddyfile.display()
            ))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?;
    }
    let mut child = tokio::process::Command::new(&caddy)
        .arg("run")
        .arg("--config")
        .arg(&caddyfile)
        .arg("--adapter")
        .arg("caddyfile")
        .arg("--watch")
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| {
            CmdError::click(format!(
                "the edge proxy {} could not start: {error}",
                caddy.display()
            ))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?;
    eprintln!(
        "[stado serve edge] {} serving {} (pid {})",
        caddy.display(),
        caddyfile.display(),
        child.id().unwrap_or_default()
    );
    let pid = child
        .id()
        .map_or_else(|| "an exited child".to_string(), |pid| format!("pid {pid}"));
    let status = crate::wait::until(
        crate::wait::Kind::Process,
        format!(
            "the edge proxy {} serving {}",
            caddy.display(),
            caddyfile.display()
        ),
        pid,
        child.wait(),
    )
    .await
    .map_err(|error| {
        CmdError::click(format!(
            "the edge proxy {} could not be waited on: {error}",
            caddy.display()
        ))
        .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    Err(CmdError::click(format!(
        "the edge proxy {} serving {} exited ({status}); no hostname is terminated until \
         com.wisent.stado restarts",
        caddy.display(),
        caddyfile.display()
    ))
    .stating(crate::primitives::failure::FailureCode::InfraDown))
}
