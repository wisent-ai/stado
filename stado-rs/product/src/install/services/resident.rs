//! The resident product's own native unit: the one service a CLI installation
//! of the running program owns. Its command line is rendered by an executable
//! of that product (`service ensure` renders from the executable's own
//! catalog), so whichever executable the unit ends up starting must be the
//! one that rendered it last; a unit rendered by one release and started by
//! another may carry an option the started one refuses, and then it restarts
//! without end.

use crate::{
    catalog::text,
    common::{capture, checked},
};
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

/// The host `program` names for itself.
fn host_of(program: &Path, label: &str) -> Result<String> {
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
    Ok(host.to_owned())
}

/// Render `label` on `host` through `program`: its own `service ensure`,
/// which writes the unit from that executable's catalog and the machine's
/// record and restarts it.
fn render_on(program: &Path, id: &str, label: &str, host: &str, reason: &str) -> Result<()> {
    let outcome = checked(std::process::Command::new(program).args([
        "service", "ensure", id, "--host", host, "--reason", reason, "--json",
    ]))?;
    eprintln!(
        "{id}: rendered {label} on {host} through {}: {}",
        program.display(),
        String::from_utf8_lossy(&outcome.stdout).trim()
    );
    Ok(())
}

/// Render `label` through `program` on the host it names for itself.
/// Returns the host.
fn render(program: &Path, id: &str, label: &str, reason: &str) -> Result<String> {
    let host = host_of(program, label)?;
    render_on(program, id, label, &host, reason)?;
    Ok(host)
}

/// Why the resident unit serves none of its declared ports now, read through
/// `program`'s `service serving` (without waiting); `None` while every
/// declared port is served.
fn not_serving(program: &Path, id: &str, host: &str) -> Result<Option<String>> {
    let output = capture(std::process::Command::new(program).args([
        "service", "serving", id, "--host", host, "--json",
    ]))?;
    let rows: Value = serde_json::from_slice(&output.stdout).with_context(|| {
        format!(
            "{id} on {host}: `service serving` through {} answered no unit rows: {}",
            program.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )
    })?;
    let rows = rows
        .as_array()
        .with_context(|| format!("{id} on {host}: `service serving` answered {rows}, not unit rows"))?;
    if !rows.is_empty() && rows.iter().all(|row| row["serving"] == "serving") {
        return Ok(None);
    }
    Ok(Some(serde_json::to_string(rows)?))
}

/// The resident unit an installation carries to the candidate release.
pub struct Resident {
    pub host: String,
    label: String,
    /// Set when the unit served nothing before the installation: it was not
    /// rendered through the candidate while the predecessor still occupied
    /// the installed path, because restarting it there starts the
    /// predecessor again, the very executable that does not serve. It is
    /// rendered through the installed release once that is placed
    /// ([`render_placed`]).
    pub stalled: Option<String>,
}

/// Reconcile a resident product's native unit through the prepared candidate
/// while the predecessor still occupies its installed path. The candidate's
/// own catalog, rather than the installed binary's old catalog, owns argv.
///
/// A unit that serves none of its declared ports is left as it is: its
/// restart would start the predecessor at the installed path again. An
/// install of Stado whose own unit hung at start (the predecessor waited on
/// a Tailscale client that never answered) failed at this step every time,
/// and the fix it carried never reached the path the unit starts.
pub fn migrate_host_unit(
    product: &Value,
    home: &Path,
    plan: &crate::install::plan::Prepared,
) -> Result<Option<Resident>> {
    let id = text(product, "id")?;
    let Some(label) = resident_label(product, home)? else {
        return Ok(None);
    };
    let candidate = placement(plan, id)?.source.as_path();
    let host = host_of(candidate, &label)?;
    if let Some(observed) = not_serving(candidate, id, &host)? {
        eprintln!(
            "{id}: {label} on {host} serves none of its declared ports ({observed}); it is rendered \
             through the installed release once that is placed, not restarted onto the release it runs now"
        );
        return Ok(Some(Resident { host, label, stalled: Some(observed) }));
    }
    render_on(
        candidate,
        id,
        &label,
        &host,
        "converge the product unit to the candidate release command line",
    )?;
    Ok(Some(Resident { host, label, stalled: None }))
}

/// Render a resident unit that served nothing before the installation
/// through `program`, the release now placed at the unit's installed path;
/// its `service ensure` restarts the unit onto it. A unit that was migrated
/// before placement is left to its own handover.
pub fn render_placed(product: &Value, resident: &Resident, program: &Path) -> Result<()> {
    if resident.stalled.is_none() {
        return Ok(());
    }
    let id = text(product, "id")?;
    render_on(
        program,
        id,
        &resident.label,
        &resident.host,
        "start the product unit on the installed release; it served nothing on the release it replaced",
    )
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
