//! Weles recordings policy: acquisition scopes and the SPIS admission
//! trust document.

pub(in crate::cli::host) mod scopes;
pub(in crate::cli::host) mod trust;

use crate::cli::CmdError;
use crate::targets::ComputeTarget;

use crate::cli::host::files::forwarding::deliver_file;
use crate::cli::host::machine::releases::release_component;
use crate::cli::host::machine::users::credentials::credential_host;
use crate::cli::host::secrets::weles::scopes::register_acquisition_scopes;

/// One scratch file in the host's own `.stado` directory, owner-only from the
/// moment `mktemp` creates it.
async fn acquisition_scratch(
    resolved: &ComputeTarget,
    home: &str,
    suffix: &str,
    runner: &crate::deploy::Runner,
) -> Result<String, String> {
    let made = crate::deploy::host_channel::run_command(
        resolved,
        &format!(
            "mktemp {}",
            crate::deploy::shlex_quote(&format!("{home}/.stado/{suffix}"))
        ),
        runner,
    )
    .await
    .map_err(|error| error.to_string())?;
    if !made.ok() {
        return Err(crate::deploy::host_channel::last_error_line(
            &made,
            "could not create a scratch file on the host",
        ));
    }
    Ok(made.stdout.trim().to_string())
}

/// Best-effort removal of this registration's scratch files — the retired
/// script's EXIT trap. A failure to remove is not a failure of the
/// registration that already happened, so it is ignored here exactly as the
/// trap's `rm -f` ignored it there.
async fn remove_remote(resolved: &ComputeTarget, paths: &[&str], runner: &crate::deploy::Runner) {
    let mut words = vec!["/bin/rm", "-f"];
    words.extend_from_slice(paths);
    let _ = crate::deploy::host_channel::run_program(resolved, &words, runner).await;
}

/// The basename a local catalog is delivered and registered under.
///
/// A name, never a path: it becomes one component under
/// `$HOME/.stado/files` on the host, so it follows the delivered-name rules
/// ([`release_component`]) and additionally may not start with `.` — no
/// hidden files, and no `.`/`..` components, whichever spelling produced
/// them.
fn catalog_file_name(source: &str) -> Result<String, CmdError> {
    let name = std::path::Path::new(source)
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| CmdError::usage("catalog path must end in a file name"))?;
    release_component("catalog file name", name)?;
    if name.starts_with('.') {
        return Err(CmdError::usage("catalog file name must not start with '.'"));
    }
    Ok(name.to_string())
}

/// `stado credentials acquisition-scopes sync --host TARGET SOURCE` — deliver the checked-in
/// Skarbiec acquisition-scope catalog to TARGET and register it against the
/// host's fleet vault.
///
/// Two audited halves and no third way in: the catalog travels through the
/// [`stream_file`] delivery channel into `$HOME/.stado/files`, owner-only
/// and checksummed on arrival, and the registration is
/// [`register_acquisition_scopes`] — there is nothing to install on the host
/// and nothing left behind but the delivered catalog. This is the reviewed
/// replacement for running weles's register script through the retired helper
/// channel.
pub async fn sync_acquisition_scopes(target: &str, source: &str) -> Result<(), CmdError> {
    let metadata = std::fs::symlink_metadata(source)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(CmdError::usage("catalog source must be a regular file"));
    }
    let name = catalog_file_name(source)?;
    let credential_host = credential_host(target).await?;
    let resolved = credential_host.target;
    let vault = credential_host.vault;
    let runner = crate::deploy::production_runner();
    let (delivered, _bytes) = deliver_file(target, source, &name).await?;
    let printed =
        register_acquisition_scopes(&resolved, &delivered, &name, &vault, &runner).await?;
    print!("{printed}");
    if !printed.ends_with('\n') {
        println!();
    }
    Ok(())
}
