//! The release inputs half of `stado quality check`.
//!
//! A gate may read a declared input: `stado web quality --git-input` answers a
//! private Git dependency from a bundle the manifest pins. The release worker
//! stages every input before the first gate and publishes each as
//! `WISENT_INPUT_<NAME>_DIR`; a check that ran the gates without them refused
//! every such product that the worker would have built, so it could never be
//! handed off. The check confirms every pinned object is in its store, and
//! stages the ones a selected gate declares it reads the way the worker
//! stages them: the bytes must hash to the pin, and they are extracted or
//! mounted as declared, beside the exported tree and removed with it. A
//! check that builds the product (installation, release submission, the
//! command tree) stages every input, as the worker stages every input before
//! the build that reads them: a build script that requires its pinned
//! dependency (Brama's `src/release/build.sh` and Skarbiec) was otherwise run
//! without it and refused every submission. An input that neither the build
//! nor a selected gate reads is confirmed stored and left in the store, so a
//! formatting check never waits for a dependency resolution it does not use.

use std::path::Path;

use super::gates::DeclaredGates;
use super::Report;
use crate::cli::CmdError;

/// Stage every input a selected gate of `declared` reads under `area`, and
/// every input when the check `builds` the product, answering the variables
/// they are read from, after confirming every pinned input is stored.
pub(super) async fn stage(
    declared: &DeclaredGates,
    area: &Path,
    builds: bool,
    report: Report,
) -> Result<Vec<(String, String)>, CmdError> {
    let mut variables = Vec::new();
    for (name, input) in &declared.inputs {
        report.say(&format!(
            "stado quality check: confirming release input {name} of {} is stored at {}",
            declared.product, input.uri
        ));
        // Asked first so an absent pin is refused as absent, naming the pin,
        // rather than as a failed download.
        crate::cli::storage::require_present(
            &input.uri,
            &format!("release input {name} of {}", declared.product),
        )
        .await?;
        let mut readers: Vec<&str> = declared
            .gates
            .iter()
            .filter(|gate| gate.inputs.iter().any(|read| read == name))
            .map(|gate| gate.name.as_str())
            .collect();
        if builds {
            readers.push("the build");
        }
        if readers.is_empty() {
            report.say(&format!(
                "stado quality check: release input {name} is stored at {}; neither the build \
                 nor a selected gate reads it, so it stays in the store",
                input.uri
            ));
            continue;
        }
        report.say(&format!(
            "stado quality check: release input {name} at {}, read by {}",
            input.uri,
            readers.join(", ")
        ));
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
