//! What the host reports: the reporter run on the host, and its wire format
//! read back.

mod artefact;
pub(in crate::cli::service_converge) mod declaration;
mod probe;
pub(in crate::cli::service_converge) mod software;
mod units;

use std::collections::BTreeMap;

use crate::deploy::{host_release, Runner};
use crate::targets::ComputeTarget;

use crate::cli::service_converge::model::vocabulary::{Installed, ATTEST_UNKNOWN, NONE};
use crate::cli::service_converge::observing::probe::probe_installed_versions;

// ---------------------------------------------------------------------------
// What the host reports
// ---------------------------------------------------------------------------

/// Every version the host reported, keyed by binary name, or the reason nothing
/// was read.
///
/// The failure is one value for the whole host on purpose: when the reporter
/// cannot run, no binary on that box has a reported version, and the same
/// sentence belongs on every row rather than one row carrying the detail and
/// the rest carrying a blank.
///
/// The reporter is [`probe_installed_versions`]: the checks the retired probe
/// script ran, as individual remote commands with every branch taken here, so
/// there is nothing to install on the host and the failure text is the
/// remote's own words, never a remedy for a delivery channel that no longer
/// exists. The declarations the probe compares against are the registry this
/// command already resolved — the same canonical registry the retired script
/// re-read on the host to learn which host it was reporting on. Reading a
/// version is a status read and nothing else, so every remote command runs
/// under the channel's ordinary read bound.
pub(super) async fn read_installed(
    target: &ComputeTarget,
    runner: &Runner,
) -> Result<BTreeMap<String, Installed>, String> {
    match probe_installed_versions(target, runner).await {
        Ok(stdout) => Ok(parse_report(&stdout)),
        Err(error) => Err(error.to_string()),
    }
}

/// The reporter's stdout, as a binary-to-report map.
///
/// Line-oriented `key=value` rather than JSON because a shell script that has
/// to emit valid JSON emits invalid JSON the first time a path contains a
/// quote. Blank lines and `#` comments are skipped, unknown keys are ignored so
/// the reporter can add fields without a matching release here, and only an
/// exact version is kept: `version=unknown` — or anything else that is not a
/// semantic version — is the reporter saying it could not tell, which is
/// [`UNKNOWN`] and never a comparison.
fn parse_report(stdout: &str) -> BTreeMap<String, Installed> {
    let mut reported = BTreeMap::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut binary = None;
        let mut entry = Installed::default();
        let mut raw_version = "";
        for token in line.split_whitespace() {
            if let Some(value) = token.strip_prefix("binary=") {
                binary = Some(value);
            } else if let Some(value) = token.strip_prefix("version=") {
                raw_version = value;
            } else if let Some(value) = token.strip_prefix("root=") {
                entry.root = value.to_string();
            } else if let Some(value) = token.strip_prefix("unit=") {
                entry.unit = value.to_string();
            } else if let Some(value) = token.strip_prefix("state=") {
                entry.state = value.to_string();
            } else if let Some(value) = token.strip_prefix("attestation=") {
                entry.attestation = value.to_string();
            } else if let Some(value) = token.strip_prefix("receipt=") {
                entry.receipt = if value == NONE {
                    String::new()
                } else {
                    value.replace('_', " ")
                };
            }
        }
        let Some(binary) = binary else {
            continue;
        };
        if host_release::is_exact_semver(raw_version) {
            entry.version = Some(raw_version.to_string());
        }
        if entry.attestation.is_empty() {
            entry.attestation = ATTEST_UNKNOWN.to_string();
        }
        reported.insert(binary.to_string(), entry);
    }
    reported
}
