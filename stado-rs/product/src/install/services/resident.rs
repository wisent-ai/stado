//! The resident product's own native unit: the one service a CLI installation
//! of the running program owns. Its command line is rendered by an executable
//! of that product (`service ensure` renders from the executable's own
//! catalog), so whichever executable the unit ends up starting must be the
//! one that rendered it last; a unit rendered by one release and started by
//! another may carry an option the started one refuses, and then it restarts
//! without end.

use crate::{catalog::text, common::checked};
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// The label of the resident unit this machine holds for `product`, when
/// `product` is the running program and one of its native unit files exists.
/// Only the product this installation runs as owns such a unit: its
/// executables are asked `registry self` and `service ensure`, verbs only
/// that program has, so a product whose id is not the running program's name
/// has none.
fn resident_label(product: &Value, home: &Path) -> Result<Option<String>> {
    let id = text(product, "id")?;
    let running = std::env::current_exe().context("reading the running program's path")?;
    if running.file_name() != Some(std::ffi::OsStr::new(id)) {
        return Ok(None);
    }
    let label = text(&product["service"], "unit")?;
    let native = [
        home.join("Library/LaunchAgents")
            .join(format!("{label}.plist")),
        PathBuf::from("/Library/LaunchDaemons").join(format!("{label}.plist")),
        home.join(".config/systemd/user")
            .join(format!("{label}.service")),
        PathBuf::from("/etc/systemd/system").join(format!("{label}.service")),
    ];
    for path in native {
        if path
            .try_exists()
            .with_context(|| format!("checking native unit {}", path.display()))?
        {
            return Ok(Some(label.to_owned()));
        }
    }
    Ok(None)
}

/// Render `label` through `program`: the host it names for itself, then its
/// own `service ensure`, which writes the unit from that executable's catalog
/// and the machine's record and restarts it. Returns the host.
fn render(program: &Path, id: &str, label: &str, reason: &str) -> Result<String> {
    let self_host = checked(std::process::Command::new(program).args([
        "registry",
        "self",
        "--local",
        "--name-only",
    ]))?;
    let host = String::from_utf8(self_host.stdout)?;
    let host = host.trim();
    if host.is_empty() {
        bail!(
            "the installed {label} unit cannot be rendered through {}: registry self --local named no host",
            program.display()
        );
    }
    let outcome = checked(std::process::Command::new(program).args([
        "service", "ensure", id, "--host", host, "--reason", reason, "--json",
    ]))?;
    eprintln!(
        "{id}: rendered {label} on {host} through {}: {}",
        program.display(),
        String::from_utf8_lossy(&outcome.stdout).trim()
    );
    Ok(host.to_owned())
}

/// Reconcile a resident product's native unit through the prepared candidate
/// while the predecessor still occupies its installed path. The candidate's
/// own catalog, rather than the installed binary's old catalog, owns argv.
pub fn migrate_host_unit(
    product: &Value,
    home: &Path,
    plan: &crate::install::plan::Prepared,
) -> Result<Option<String>> {
    let id = text(product, "id")?;
    let Some(label) = resident_label(product, home)? else {
        return Ok(None);
    };
    let candidate = placement(plan, id)?.source.as_path();
    render(
        candidate,
        id,
        &label,
        "converge the product unit to the candidate release command line",
    )
    .map(Some)
}

/// Render the resident unit again through `program`, the executable now at
/// the unit's installed path, after an installation that migrated the unit
/// stopped before completing or a rollback put an earlier release back. The
/// unit then carries only what the executable it starts accepts. `None` when
/// this machine holds no resident unit of `product`.
pub fn restore_host_unit(product: &Value, home: &Path, program: &Path) -> Result<Option<String>> {
    let id = text(product, "id")?;
    let Some(label) = resident_label(product, home)? else {
        return Ok(None);
    };
    render(
        program,
        id,
        &label,
        "converge the product unit to the command line of the installed release",
    )
    .map(Some)
}

/// `failed`, the error that stopped an installation after it migrated the
/// resident unit to the candidate's command line, with what rendering the
/// unit again through `program` (the executable left at its installed path)
/// did: the unit then starts what is installed, or the refusal says it may
/// not.
pub fn restored_after(
    product: &Value,
    home: &Path,
    program: &Path,
    failed: anyhow::Error,
) -> anyhow::Error {
    match restore_host_unit(product, home, program) {
        Ok(_) => failed.context(format!(
            "the installation stopped after migrating the resident unit to the candidate's command line; \
             the unit was rendered again through {}, which is still installed",
            program.display()
        )),
        Err(restoring) => failed.context(format!(
            "the installation stopped after migrating the resident unit to the candidate's command line, \
             and rendering it again through {} failed too, so the unit may carry options that executable \
             refuses: {restoring:#}",
            program.display()
        )),
    }
}

/// The placement of `product`'s own executable in the prepared installation:
/// its prepared `source` and the installed `destination` it replaces.
fn placement<'a>(
    plan: &'a crate::install::plan::Prepared,
    product: &str,
) -> Result<&'a crate::install::plan::Placement> {
    plan.placements
        .iter()
        .find(|placed| {
            !placed.symbolic
                && placed
                    .destination
                    .file_name()
                    .is_some_and(|name| name == product)
        })
        .with_context(|| {
            format!(
                "{product}: prepared installation has no executable to check against managed units"
            )
        })
}

/// Where the prepared installation places `product`'s executable: the path
/// the resident unit starts.
pub fn installed_binary(plan: &crate::install::plan::Prepared, product: &str) -> Result<PathBuf> {
    Ok(placement(plan, product)?.destination.clone())
}

/// Parse the native unit's actual arguments with the prepared executable
/// before any installed bytes are replaced. Migration above has already
/// rewritten a catalog-owned unit while the old executable still answers.
pub fn admit_units(plan: &crate::install::plan::Prepared, product: &str, host: &str) -> Result<()> {
    let candidate = placement(plan, product)?.source.as_path();
    let output = checked(
        std::process::Command::new(candidate)
            .args(["quality", "units", "--candidate"])
            .arg(candidate)
            .args(["--program", product, "--host", host]),
    )?;
    eprintln!(
        "{product}: {}",
        String::from_utf8_lossy(&output.stdout).trim()
    );
    Ok(())
}
