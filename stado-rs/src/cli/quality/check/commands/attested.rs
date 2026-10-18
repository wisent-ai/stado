//! What the CLI command-tree audit stands on when its model reading obtained
//! no judgment: the attestation the checked commit carries, read word for
//! word by Tama (`tama attestation check --commit`), with no model.
//!
//! A product works while another product is down, and where Brama's judge
//! does not answer, the working model's own attestation settles the question
//! the judge would have answered. The audit used to refuse every revision for
//! as long as Brama did not answer, so no delivery pass could install
//! anything; now an unjudged tree passes only on a commit whose attestation
//! stands, and every refusal names both why the judge was not obtained and
//! what the attestation lacks.

use std::path::Path;
use std::process::Command;

use serde_json::Value;

use crate::cli::CmdError;
use crate::deploy::host_exec::{installed_program, TAMA_CLI};

/// Whether `revision` of the checkout at `checkout` carries an attestation
/// that stands, as the Tama a Stado install places reads it. `unjudged` says
/// why the model reading obtained no judgment; every refusal carries it.
pub(super) fn stands(checkout: &Path, revision: &str, unjudged: &str) -> Result<(), CmdError> {
    let tama = installed_program(TAMA_CLI).map_err(|missing| {
        CmdError::unreachable(format!(
            "{unjudged}; the attestation of {revision} cannot be read either: {missing}"
        ))
    })?;
    let shown = format!(
        "{} attestation check --commit {revision} --repository {} --json",
        tama.display(),
        checkout.display()
    );
    let output = crate::wait::output(
        Command::new(&tama)
            .args(["attestation", "check", "--commit", revision, "--repository"])
            .arg(checkout)
            .arg("--json"),
    )
    .map_err(|cause| CmdError::click(format!("{unjudged}; `{shown}` did not run: {cause}")))?;
    let answer: Value = serde_json::from_slice(&output.stdout).map_err(|cause| {
        CmdError::refused(format!(
            "{unjudged}; `{shown}` exited {} without a JSON answer ({cause}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    })?;
    if output.status.success() && answer["stands"].as_bool() == Some(true) {
        return Ok(());
    }
    Err(CmdError::refused(format!(
        "{unjudged}; and the attestation of {revision} does not stand (`{shown}` exited {}): \
         missing {}, findings {}",
        output.status, answer["missing"], answer["findings"]
    )))
}
