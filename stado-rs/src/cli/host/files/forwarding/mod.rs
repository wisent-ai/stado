//! Forward one file or one secret to a target account.

pub(in crate::cli::host) mod secret;
pub(in crate::cli::host) mod stream;

use crate::cli::CmdError;

use crate::cli::host::checks::probes::{cell, print_json};
use crate::cli::host::files::forwarding::secret::transfer_secret;
use crate::cli::host::files::forwarding::stream::stream_file;

/// Where a delivered file lands, relative to the target account's home.
///
/// Separate from `.stado` itself so a delivery can never take the name of a
/// credential, a helper, or anything else Stado keeps there.
pub(in crate::cli::host) const DELIVERED_FILES_DIR: &str = ".stado/files";

pub(crate) async fn install_secret_value_at_home(
    target: &str,
    name: &str,
    value: &str,
    home: &str,
) -> Result<(String, usize), CmdError> {
    transfer_secret(target, name, value.as_bytes(), Some(home)).await
}

/// Deliver one file through the [`stream_file`] channel and RETURN where it
/// landed, for a caller that renders its own report.
///
/// A callee that prints is unusable from a machine-readable caller:
/// `stado route placement publish --json` would put a delivery report in
/// front of its own document and hand the operator two JSON objects on one
/// stream. Same channel, same checksum, same owner-only mode — only the
/// reporting belongs to whoever asked.
pub(crate) async fn deliver_file(
    target: &str,
    source: &str,
    name: &str,
) -> Result<(String, usize), CmdError> {
    stream_file(target, source, name, DELIVERED_FILES_DIR, "u=rw,go=").await
}

/// `stado host deliver TARGET SOURCE DESTINATION [--files-from PATH] [--json]`
/// — atomically replace one managed run input with local bytes.
///
/// A `-` file list is read as NUL-delimited UTF-8 from stdin. It remains stdin
/// to rsync rather than becoming argv, so a tracked or untracked checkout can
/// select its exact files without exposing names to a remote shell.
pub async fn deliver(
    target: &str,
    source: &str,
    destination: &str,
    files_from: Option<&str>,
    json_output: bool,
) -> Result<(), CmdError> {
    let file_list = match files_from {
        Some("-") => {
            let mut value = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut value)?;
            Some(value)
        }
        Some(path) => Some(std::fs::read_to_string(path)?),
        None => None,
    };
    let report = crate::deploy::host_delivery::deliver_host(
        target,
        source,
        destination,
        file_list.as_deref(),
        &crate::deploy::production_runner(),
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if json_output {
        print_json(&report);
    } else {
        println!(
            "{}: delivered {} {} -> {}",
            cell(report.get("target")),
            cell(report.get("kind")),
            cell(report.get("source")),
            cell(report.get("destination")),
        );
    }
    Ok(())
}
