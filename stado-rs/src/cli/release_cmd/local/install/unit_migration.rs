//! Reconcile the native host unit with the incoming binary's catalog while
//! the predecessor still occupies the installed executable path. A rejected
//! argv never reaches the replacement rename.

use std::path::Path;
use std::process::Command;

use crate::cli::CmdError;

pub(super) fn admit(
    candidate: &Path,
    product: &str,
    home: &Path,
) -> Result<Option<String>, CmdError> {
    if product != "stado" {
        return Ok(None);
    }
    let label = crate::deploy::service_catalog::host_unit().map_err(CmdError::declaration)?;
    let native = [
        home.join("Library/LaunchAgents")
            .join(format!("{label}.plist")),
        Path::new("/Library/LaunchDaemons").join(format!("{label}.plist")),
        home.join(".config/systemd/user")
            .join(format!("{label}.service")),
        Path::new("/etc/systemd/system").join(format!("{label}.service")),
    ];
    let mut present = false;
    for path in native {
        present |= path.try_exists().map_err(|cause| {
            CmdError::unreachable(format!(
                "cannot inspect native unit {} before installing {product}: {cause}",
                path.display()
            ))
        })?;
    }
    if !present {
        return Ok(None);
    }
    let self_host = crate::wait::output(Command::new(candidate).args([
        "registry",
        "self",
        "--local",
        "--name-only",
    ]))
    .map_err(|cause| {
        CmdError::unreachable(format!(
            "candidate {} cannot identify this unit's host: {cause}",
            candidate.display()
        ))
    })?;
    if !self_host.status.success() {
        return Err(CmdError::refused(format!(
            "candidate {} cannot identify the host of {label} ({}): {} {}",
            candidate.display(),
            self_host.status,
            String::from_utf8_lossy(&self_host.stdout).trim(),
            String::from_utf8_lossy(&self_host.stderr).trim()
        )));
    }
    let hostname = String::from_utf8_lossy(&self_host.stdout);
    let host = hostname.trim();
    if host.is_empty() {
        return Err(CmdError::refused(format!(
            "candidate {} named no registry host for {label}",
            candidate.display()
        )));
    }
    let ensure = crate::wait::output(Command::new(candidate).args([
        "service",
        "ensure",
        product,
        "--host",
        host,
        "--reason",
        "converge the resident product unit to the incoming release",
        "--json",
    ]))
    .map_err(|cause| {
        CmdError::unreachable(format!(
            "candidate {} cannot migrate {label} on {host}: {cause}",
            candidate.display()
        ))
    })?;
    if !ensure.status.success() {
        return Err(CmdError::refused(format!(
            "candidate {} did not migrate {label} on {host} ({}): {} {}",
            candidate.display(),
            ensure.status,
            String::from_utf8_lossy(&ensure.stdout).trim(),
            String::from_utf8_lossy(&ensure.stderr).trim()
        )));
    }
    println!(
        "{product}: migrated {label} on {host} before replacement: {}",
        String::from_utf8_lossy(&ensure.stdout).trim()
    );
    let compatible = crate::wait::output(
        Command::new(candidate)
            .args(["quality", "units", "--candidate"])
            .arg(candidate)
            .args(["--program", product, "--host", host]),
    )
    .map_err(|cause| {
        CmdError::unreachable(format!(
            "candidate {} cannot check {label} on {host}: {cause}",
            candidate.display()
        ))
    })?;
    if !compatible.status.success() {
        return Err(CmdError::refused(format!(
            "candidate {} refused managed unit argv on {host} ({}): {} {}",
            candidate.display(),
            compatible.status,
            String::from_utf8_lossy(&compatible.stdout).trim(),
            String::from_utf8_lossy(&compatible.stderr).trim()
        )));
    }
    println!(
        "{product}: {}",
        String::from_utf8_lossy(&compatible.stdout).trim()
    );
    Ok(Some(host.to_string()))
}
