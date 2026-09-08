//! The delivery contract's local endpoint: installing a verified archive on
//! this very host and reconciling the readers that execute what it replaced.

use std::path::{Path, PathBuf};

use clap::Args;
use serde_json::{json, Value};

use crate::cli::CmdError;

pub(super) mod converge;
pub(super) mod install;

/// `stado release install-local` — the delivery contract's local endpoint.
///
/// A delivery job pinned to its target runs ON that target, so installation
/// is a local file operation and needs no login service: the release that
/// installed over ssh died on the first host without Remote Login. The
/// archive path and digest come from the delivery worker's environment
/// (`WISENT_RELEASE_ARCHIVE`, `WISENT_RELEASE_SHA256`), the same contract
/// the retired python installer read. This command replaced the last
/// load-bearing script of the 137 deleted on 2026-08-19.
#[derive(Args)]
pub struct ReleaseInstallLocalArgs {
    /// Archive member to install, e.g. bin/stado.
    #[arg(long, default_value = "bin/stado")]
    member: String,
    /// Installed name under $HOME/.stado/bin; defaults to the member's
    /// basename.
    #[arg(long, default_value = "")]
    name: String,
}

#[derive(Args)]
pub struct ReleaseConvergeLocalReadersArgs {
    #[arg(long, default_value = "stado")]
    name: String,
    /// Verified release archive retained by the root product delivery.
    #[arg(long)]
    archive: PathBuf,
    /// Catalog SHA-256 for `archive`.
    #[arg(long)]
    sha256: String,
}

/// Install the same verified Stado archive into every registry-declared
/// service-local Stado reader on this host.
///
/// `install-local` historically replaced only `$HOME/.stado/bin/stado`.
/// Services such as the mini's coordinator execute an independently installed
/// `.../.stado/services/<service>/current/darwin-arm/stado`, so the native
/// delivery could report success while that long-running reader kept parsing
/// the registry with an older schema. Invoke the existing `service update`
/// operation with this delivery's exact archive rather than duplicating its
/// checked install, relink, declared lifecycle, and kernel-image verification.
async fn converge_service_local_stado_readers(
    context: &str,
    executable: &Path,
    archive: &Path,
) -> Result<(), CmdError> {
    let registry = crate::cli::registry::read_registry().await?;
    let hostname = crate::providers::vast::system_hostname();
    let target = registry
        .lookup_self(&hostname)
        .map_err(|error| CmdError::click(error.to_string()))?
        .ok_or_else(|| {
            CmdError::click(format!(
                "{context}: no registry target names this machine ({hostname})"
            ))
        })?;
    let mut readers: Vec<String> = crate::deploy::service::declared_services(target)
        .into_iter()
        .filter(|service| service.source == crate::deploy::service::SOURCE_REGISTRY)
        .filter(crate::deploy::service::is_service_local_stado_reader)
        .map(|service| service.name)
        .collect();
    readers.sort();
    for pair in readers.windows(2) {
        if pair[0] == pair[1] {
            return Err(CmdError::click(format!(
                "{context}: registry target {} declares service-local Stado reader {} more than once",
                target.name, pair[0]
            )));
        }
    }
    if !executable.is_file() {
        return Err(CmdError::click(format!(
            "{context}: installed Stado executable {} is unavailable",
            executable.display()
        )));
    }
    for reader in readers {
        println!(
            "{context}: converging service-local Stado reader {} on {}",
            reader, target.name
        );
        let output = tokio::process::Command::new(executable)
            .args([
                "service",
                "update",
                &reader,
                "--host",
                &target.name,
                "--from-archive",
            ])
            .arg(archive)
            .args(["--refresh-image", "--json"])
            .output()
            .await
            .map_err(|error| {
                CmdError::click(format!(
                    "{context}: cannot start service-local reader convergence for {reader}: {error}"
                ))
            })?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        print!("{stdout}");
        eprint!("{stderr}");
        if !output.status.success() {
            let captured_stdout = serde_json::from_str::<Value>(stdout.trim())
                .unwrap_or_else(|_| json!(stdout.trim()));
            let captured = json!({
                "exit_code": output.status.code(),
                "stderr": stderr.trim(),
                "stdout": captured_stdout,
            });
            return Err(CmdError::click(format!(
                "{context}: service-local Stado reader {reader} on {} did not converge: {captured}",
                target.name
            )));
        }
    }
    Ok(())
}

/// Compare an installed executable with the already-verified archive payload
/// without allocating a second copy of either file.
fn regular_file_matches(path: &Path, expected: &[u8]) -> Result<bool, CmdError> {
    use std::io::Read as _;

    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(CmdError::click(format!(
                "cannot inspect installed executable {}: {error}",
                path.display()
            )))
        }
    };
    if !metadata.file_type().is_file() || metadata.len() != expected.len() as u64 {
        return Ok(false);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Ok(false);
        }
    }
    let mut installed = std::fs::File::open(path).map_err(|error| {
        CmdError::click(format!(
            "cannot read installed executable {}: {error}",
            path.display()
        ))
    })?;
    let mut offset = 0usize;
    let mut buffer = [0_u8; 128 * 1024];
    loop {
        let count = installed.read(&mut buffer).map_err(|error| {
            CmdError::click(format!(
                "cannot compare installed executable {}: {error}",
                path.display()
            ))
        })?;
        if count == 0 {
            return Ok(offset == expected.len());
        }
        let end = offset + count;
        if expected.get(offset..end) != Some(&buffer[..count]) {
            return Ok(false);
        }
        offset = end;
    }
}
