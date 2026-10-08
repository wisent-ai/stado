//! The release inputs half of `stado quality check`.
//!
//! A gate may read a declared input: `stado web quality --git-input` answers a
//! private Git dependency from a bundle the manifest pins. The release worker
//! stages every input before the first gate and publishes each as
//! `WISENT_INPUT_<NAME>_DIR`; a check that ran the gates without them refused
//! every such product that the worker would have built, so it could never be
//! handed off. The check stages them the same way: each pinned object must be
//! in its store, its bytes must hash to the pin, and it is extracted or
//! mounted as declared, beside the exported tree and removed with it.

use std::path::Path;

use super::gates::FormatGates;
use super::Report;
use crate::cli::CmdError;

/// Stage every input `declared` pins under `area`, answering the variables a
/// gate reads them from.
pub(super) async fn stage(
    declared: &FormatGates,
    area: &Path,
    report: Report,
) -> Result<Vec<(String, String)>, CmdError> {
    let mut variables = Vec::new();
    for (name, input) in &declared.inputs {
        report.say(&format!(
            "stado quality check: release input {name} at {}",
            input.uri
        ));
        // Asked first so an absent pin is refused as absent, naming the pin,
        // rather than as a failed download.
        crate::cli::storage::require_present(
            &input.uri,
            &format!("release input {name} of {}", declared.product),
        )
        .await?;
        let bytes = crate::cli::storage::fetch_object(&input.uri).await?;
        let digest = crate::release_control::sha256_bytes(&bytes);
        if digest != input.sha256 {
            return Err(CmdError::refused(format!(
                "release input {name} of {} at {} hashes to {digest}, but the manifest pins {}",
                declared.product, input.uri, input.sha256
            )));
        }
        let destination = area.join(&input.mount);
        if input.extract {
            crate::release_control::safe_extract_archive(&bytes, &destination)
                .map_err(CmdError::refused)?;
        } else {
            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent).map_err(|error| {
                    CmdError::click(format!("cannot create {}: {error}", parent.display()))
                        .stating(crate::cli::entry::error::io_failure_code(error.kind()))
                })?;
            }
            std::fs::write(&destination, &bytes).map_err(|error| {
                CmdError::click(format!("cannot write {}: {error}", destination.display()))
                    .stating(crate::cli::entry::error::io_failure_code(error.kind()))
            })?;
        }
        variables.push((
            crate::cli::release_submit::input_variable(name),
            destination.display().to_string(),
        ));
    }
    Ok(variables)
}
