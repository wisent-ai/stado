//! Qualify a staged native executable against the real unit files on every
//! registry host of its platform, before a release publishes those bytes.

use std::path::Path;
use std::process::Command;

use crate::cli::quality::{gates::DeclaredGates, Report};
use crate::cli::CmdError;

pub(crate) fn check(
    binary: &Path,
    declared: &DeclaredGates,
    report: Report,
) -> Result<(), CmdError> {
    if declared.product != "stado" {
        return Ok(());
    }
    let output = crate::wait::output(
        Command::new(binary)
            .args(["quality", "units", "--candidate"])
            .arg(binary)
            .args([
                "--program",
                &declared.product,
                "--platform",
                &declared.platform,
            ]),
    )
    .map_err(|cause| {
        CmdError::unreachable(format!(
            "cannot run {} unit compatibility gate: {cause}",
            binary.display()
        ))
    })?;
    if !output.status.success() {
        return Err(CmdError::refused(format!(
            "{} {} unit compatibility gate refused before release publication: {}: stdout: {}; stderr: {}",
            declared.product, declared.platform, output.status,
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    report.say(String::from_utf8_lossy(&output.stdout).trim());
    Ok(())
}
