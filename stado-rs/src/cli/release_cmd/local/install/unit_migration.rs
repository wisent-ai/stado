//! Reconcile the native host unit with the incoming binary's catalog while
//! the predecessor still occupies the installed executable path. A rejected
//! argv never reaches the replacement rename. The unit is rendered by an
//! executable's own `service ensure`, so whenever the install stops after the
//! migration, or puts the predecessor back, the unit is rendered again
//! through the executable left installed: a unit rendered by the incoming
//! release may carry an option the predecessor refuses, and the predecessor
//! would then restart on it without end.

use std::path::Path;
use std::process::Command;

use crate::cli::CmdError;

/// The resident unit's label when `product` runs as this machine's resident
/// unit and one of its native unit files exists. Only the host process runs
/// as that unit: the product is the one the catalog names as the host
/// process, never a name written here.
fn resident_label(product: &str, home: &Path) -> Result<Option<String>, CmdError> {
    let host = crate::deploy::service_catalog::host_process().map_err(CmdError::declaration)?;
    if product != host.name {
        return Ok(None);
    }
    let label = crate::deploy::service_catalog::host_unit().map_err(CmdError::declaration)?;
    for path in crate::deploy::service::resident_unit_paths(home, &label) {
        let present = path.try_exists().map_err(|cause| {
            CmdError::unreachable(format!(
                "cannot inspect native unit {} of {product}: {cause}",
                path.display()
            ))
        })?;
        if present {
            return Ok(Some(label));
        }
    }
    Ok(None)
}

/// The registry host `program` names for itself, where `label` runs.
fn host_of(program: &Path, label: &str) -> Result<String, CmdError> {
    let self_host = crate::wait::output(Command::new(program).args([
        "registry",
        "self",
        "--local",
        "--name-only",
    ]))
    .map_err(|cause| {
        CmdError::unreachable(format!(
            "{} cannot identify this unit's host: {cause}",
            program.display()
        ))
    })?;
    if !self_host.status.success() {
        return Err(CmdError::refused(format!(
            "{} cannot identify the host of {label} ({}): {} {}",
            program.display(),
            self_host.status,
            String::from_utf8_lossy(&self_host.stdout).trim(),
            String::from_utf8_lossy(&self_host.stderr).trim()
        )));
    }
    let hostname = String::from_utf8_lossy(&self_host.stdout);
    let host = hostname.trim();
    if host.is_empty() {
        return Err(CmdError::refused(format!(
            "{} named no registry host for {label}",
            program.display()
        )));
    }
    Ok(host.to_string())
}

/// Write `label` on `host` through `program`'s own `service ensure`, which
/// renders the unit from that executable's catalog and restarts it.
fn ensure(
    program: &Path,
    product: &str,
    label: &str,
    host: &str,
    reason: &str,
) -> Result<(), CmdError> {
    let ensure = crate::wait::output(Command::new(program).args([
        "service", "ensure", product, "--host", host, "--reason", reason, "--json",
    ]))
    .map_err(|cause| {
        CmdError::unreachable(format!(
            "{} cannot render {label} on {host}: {cause}",
            program.display()
        ))
    })?;
    if !ensure.status.success() {
        return Err(CmdError::refused(format!(
            "{} did not render {label} on {host} ({}): {} {}",
            program.display(),
            ensure.status,
            String::from_utf8_lossy(&ensure.stdout).trim(),
            String::from_utf8_lossy(&ensure.stderr).trim()
        )));
    }
    println!(
        "{product}: rendered {label} on {host} through {}: {}",
        program.display(),
        String::from_utf8_lossy(&ensure.stdout).trim()
    );
    Ok(())
}

/// Render `label` through `program` on the host it names for itself, and
/// return that host.
fn render(program: &Path, product: &str, label: &str, reason: &str) -> Result<String, CmdError> {
    let host = host_of(program, label)?;
    ensure(program, product, label, &host, reason)?;
    Ok(host)
}

/// Migrate the resident unit through `candidate`, then have `candidate` parse
/// the unit's actual arguments. A failed write or a refused check renders the
/// unit again through `installed`, the executable still at the unit's
/// installed path; a host the candidate cannot name has written nothing.
pub(super) fn admit(
    candidate: &Path,
    installed: &Path,
    product: &str,
    home: &Path,
) -> Result<Option<String>, CmdError> {
    let Some(label) = resident_label(product, home)? else {
        return Ok(None);
    };
    let host = host_of(candidate, &label)?;
    let reason = "converge the resident product unit to the incoming release";
    if let Err(refusal) = ensure(candidate, product, &label, &host, reason) {
        return Err(restored_after(installed, product, home, refusal));
    }
    let refusal = match crate::wait::output(
        Command::new(candidate)
            .args(["quality", "units", "--candidate"])
            .arg(candidate)
            .args(["--program", product, "--host", &host]),
    ) {
        Err(cause) => CmdError::unreachable(format!(
            "candidate {} cannot check {label} on {host}: {cause}",
            candidate.display()
        )),
        Ok(compatible) if !compatible.status.success() => CmdError::refused(format!(
            "candidate {} refused managed unit argv on {host} ({}): {} {}",
            candidate.display(),
            compatible.status,
            String::from_utf8_lossy(&compatible.stdout).trim(),
            String::from_utf8_lossy(&compatible.stderr).trim()
        )),
        Ok(compatible) => {
            println!(
                "{product}: {}",
                String::from_utf8_lossy(&compatible.stdout).trim()
            );
            return Ok(Some(host));
        }
    };
    Err(restored_after(installed, product, home, refusal))
}

/// Render the resident unit again through `installed`, the executable now at
/// its installed path, so the unit carries only what that executable
/// accepts. `None` when this machine holds no resident unit of `product`.
pub(super) fn restore(
    installed: &Path,
    product: &str,
    home: &Path,
) -> Result<Option<String>, CmdError> {
    let Some(label) = resident_label(product, home)? else {
        return Ok(None);
    };
    render(
        installed,
        product,
        &label,
        "converge the resident product unit to the installed release",
    )
    .map(Some)
}

/// `refusal`, which stopped the install after it migrated the resident unit,
/// with what rendering the unit again through `installed` did.
pub(super) fn restored_after(
    installed: &Path,
    product: &str,
    home: &Path,
    refusal: CmdError,
) -> CmdError {
    match restore(installed, product, home) {
        Ok(_) => refusal.also(format!(
            "the resident unit was rendered again through {}, which stays installed",
            installed.display()
        )),
        Err(restoring) => refusal.also(format!(
            "rendering the resident unit again through {} failed too, so the unit may carry \
             options that executable refuses: {restoring}",
            installed.display()
        )),
    }
}
