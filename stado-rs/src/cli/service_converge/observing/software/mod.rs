//! The host's software report, refreshed by the same visit that judges drift.
//!
//! `stado release status` judges every rollout target against the newest
//! software report on file and never contacts a host itself, so something has
//! to write that report. `stado host software` did until the host verbs
//! collapsed into the release capability on 2026-09-06 and the verb was
//! deleted with no writer put in its place: four days later every target read
//! `reported stale (4d)` and the sentence beside it sent operators to a
//! command that answered `Usage: stado host <COMMAND>`. The live read that
//! replaced the verb is `stado release host-state`, so the report is written
//! here, on the visit that command already makes.
//!
//! The population is the one [`crate::host_software::gather`] documents —
//! `$HOME/.stado/bin`, every declared unit's program — plus two sets only this
//! caller can name: the release-control products rolled out to the target,
//! which live under their own install roots, and the artefact roots the drift
//! reporter just resolved, so a managed binary installed outside both is still
//! rowed rather than absent.

use std::collections::BTreeMap;

use crate::deploy::Runner;
use crate::host_software::Report;
use crate::targets::ComputeTarget;

use crate::cli::service_converge::model::vocabulary::Installed;

/// Refresh TARGET's software report and return it as the store now holds it.
///
/// The registry document is read here rather than threaded through the
/// converge, because the drift verdict never needs it: `managed_versions`
/// travels on the resolved target, while the products a rollout installs are
/// declared under `release_control`, which only this step consults.
///
/// A refusal is an answer, not an error. The channel refusing to read is
/// recorded by [`crate::host_software::refresh`] as the report's own state;
/// the registry document being unreadable and the store refusing the write
/// come back as the sentence in `Err`, for the caller to print beside the
/// verdict. None of them changes the drift verdict this visit already
/// produced: the exit code of `host-state` is the documented gate on drift,
/// and "the report could not be refreshed" is a finding `release status` will
/// show as `unverified` rather than a second reason to fail a command whose
/// first answer may be fine.
pub(in crate::cli::service_converge) async fn refresh_software(
    target: &ComputeTarget,
    reported: Option<&BTreeMap<String, Installed>>,
    runner: &Runner,
) -> Result<Report, String> {
    let document = crate::cli::registry::fetch_document()
        .await
        .map_err(|error| error.to_string())?;
    let mut programs: Vec<String> =
        crate::host_software::products_rolled_out_to(&document, &target.name)
            .into_iter()
            .map(|product| product.path)
            .collect();
    programs.extend(
        reported
            .into_iter()
            .flat_map(BTreeMap::values)
            .map(|entry| entry.root.clone())
            .filter(|root| !root.is_empty()),
    );
    crate::host_software::refresh(target, &programs, runner)
        .await
        .map_err(|error| error.0)
}
