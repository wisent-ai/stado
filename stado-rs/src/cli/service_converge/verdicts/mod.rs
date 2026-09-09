//! The verdict two versions imply, and the process columns attached to it.

pub(in crate::cli::service_converge) mod ordering;
pub(in crate::cli::service_converge) mod reporting;

use std::cmp::Ordering;
use std::collections::BTreeMap;

use crate::deploy::service;
use crate::deploy::Runner;
use crate::targets::ComputeTarget;

use crate::cli::service_converge::model::vocabulary::{
    Installed, Row, ATTEST_ABSENT, ATTEST_DIFFERS, ATTEST_NEVER_DELIVERED, ATTEST_UNKNOWN,
    HOST_AHEAD, HOST_BEHIND, HOST_MISSING, IN_SYNC, NONE, UNATTESTED, UNKNOWN, VERSION_HELPER,
};
use crate::cli::service_converge::verdicts::ordering::version_order;

/// One row per declared binary, each carrying the verdict its two versions
/// imply.
pub(super) fn verdict_rows(
    declared: &[(String, String)],
    reported: &Result<BTreeMap<String, Installed>, String>,
) -> Vec<Row> {
    declared
        .iter()
        .map(|(binary, declared_version)| {
            let entry = match reported {
                Ok(reported) => reported.get(binary),
                Err(_) => None,
            };
            let installed = entry.and_then(|entry| entry.version.clone());
            let attestation = entry
                .map(|entry| entry.attestation.as_str())
                .unwrap_or(ATTEST_UNKNOWN);
            // Provenance is judged before drift, because drift between a
            // declaration and bytes nobody delivered is not the finding: the
            // bytes are. Reading these as `host-ahead` is what let a local
            // build offer to promote its own version into the registry.
            let unattested = match attestation {
                ATTEST_ABSENT => Some(format!(
                    "the host runs {} and no delivered copy of {} is staged at \
                     $HOME/.stado/releases, though earlier versions of this binary were \
                     delivered here; these bytes were put at the install path beside the \
                     delivery path, and --apply will not move the declaration to a version \
                     it cannot attest",
                    installed.as_deref().unwrap_or(UNKNOWN),
                    installed.as_deref().unwrap_or(UNKNOWN)
                )),
                ATTEST_NEVER_DELIVERED => Some(format!(
                    "the host runs {} and this binary has never been delivered here: \
                     $HOME/.stado/releases holds no version of it at all. The bootstrap \
                     installer stages nothing, so this is the expected reading for a host \
                     that has not had a release delivery yet — it is not evidence that \
                     anything was replaced",
                    installed.as_deref().unwrap_or(UNKNOWN)
                )),
                ATTEST_DIFFERS => Some(format!(
                    "the host runs {} and the staged copy of {} does not match the installed \
                     file byte for byte; the binary was replaced after delivery",
                    installed.as_deref().unwrap_or(UNKNOWN),
                    installed.as_deref().unwrap_or(UNKNOWN)
                )),
                _ => None,
            };
            if let Some(detail) = unattested {
                return Row {
                    binary: binary.clone(),
                    declared: declared_version.clone(),
                    installed,
                    verdict: UNATTESTED,
                    detail,
                    root: entry.map(|entry| entry.root.clone()).unwrap_or_default(),
                    unit: entry.map(|entry| entry.unit.clone()).unwrap_or_default(),
                    state: entry.map(|entry| entry.state.clone()).unwrap_or_default(),
                    attestation: attestation.to_string(),
                    receipt: entry
                        .map(|entry| entry.receipt.clone())
                        .filter(|receipt| !receipt.is_empty())
                        .unwrap_or_else(|| String::from(NONE)),
                    running_binary: None,
                    binary_matches_process: None,
                };
            }
            let (verdict, detail) = match (&installed, reported) {
                (Some(version), _) if version == declared_version => (
                    IN_SYNC,
                    // Provenance, when the host kept a record of it. "-" still
                    // means attested-by-bytes with no receipt, which is every
                    // delivery made before the receipt format.
                    entry
                        .map(|entry| entry.receipt.clone())
                        .filter(|receipt| !receipt.is_empty())
                        .unwrap_or_else(|| String::from("-")),
                ),
                (Some(version), _) => match version_order(version, declared_version) {
                    // The host is behind the declaration: `--apply` delivers
                    // the declared one, which is an upgrade here.
                    Some(Ordering::Less) => (
                        HOST_BEHIND,
                        format!(
                            "the host runs {version}, older than the declared \
                             {declared_version}; --apply delivers the declared one \
                             through `stado release host-state`"
                        ),
                    ),
                    // The declaration is behind the host: delivering it would
                    // be a downgrade, so nothing is delivered and the remedy
                    // is to move the declaration.
                    Some(Ordering::Greater) => (
                        HOST_AHEAD,
                        format!(
                            "the host runs {version}, newer than the declared \
                             {declared_version}: the declaration is stale, not the \
                             host; --apply refuses to downgrade it and names the \
                             declare-version command that moves the declaration"
                        ),
                    ),
                    // Equal orderings of unequal strings cannot happen for two
                    // exact semantic versions, and are reported as in sync
                    // rather than invented into drift if one ever does.
                    Some(Ordering::Equal) => (IN_SYNC, String::from("-")),
                    // Unreachable for two exact semantic versions; reported
                    // unmeasured rather than ordered by invention.
                    None => (
                        UNKNOWN,
                        format!(
                            "the host runs {version} against the declared \
                             {declared_version}, and the two cannot be ordered"
                        ),
                    ),
                },
                (None, Err(failure)) => (UNKNOWN, failure.clone()),
                // The reporter answered and found no artefact at all. Its own
                // verdict, not `unknown`: this measurement succeeded, and what
                // it measured is a host that declares a binary it does not
                // carry. `--apply` delivers it, because there is nothing here
                // to downgrade and no process running the declared binary to
                // interrupt.
                (None, Ok(_))
                    if entry
                        .map(|entry| entry.root.is_empty() || entry.root == NONE)
                        .unwrap_or_default() =>
                {
                    (
                        HOST_MISSING,
                        format!(
                            "{VERSION_HELPER} found no installed artefact for this \
                             binary on this host; --apply delivers the declared \
                             version through `stado release host-state`"
                        ),
                    )
                }
                (None, Ok(_)) => (
                    UNKNOWN,
                    match entry {
                        // The reporter found the artefact and could not read a
                        // version out of it. Said in full, because the remedy
                        // is to make the product stamp its own artefact, not
                        // to re-run this command.
                        Some(entry) => format!(
                            "{VERSION_HELPER} found {} and no version metadata in it \
                             (package.json, .weles-release, provenance.json), so this \
                             host cannot be shown to run the declared version",
                            entry.root
                        ),
                        // Nothing came back for this binary at all, which is
                        // not the same as an answer of "absent": the reporter
                        // may never have looked. Unmeasured, and never
                        // delivered on that basis.
                        None => format!(
                            "{VERSION_HELPER} reported nothing for this binary; it is \
                             not installed on this host, or the reporter could not find it"
                        ),
                    },
                ),
            };
            let cell = |value: Option<&str>| match value {
                Some(value) if !value.is_empty() => value.to_string(),
                _ => String::from(NONE),
            };
            Row {
                binary: binary.clone(),
                declared: declared_version.clone(),
                installed,
                root: cell(entry.map(|entry| entry.root.as_str())),
                unit: cell(entry.map(|entry| entry.unit.as_str())),
                state: cell(entry.map(|entry| entry.state.as_str())),
                attestation: entry
                    .map(|entry| entry.attestation.clone())
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| String::from(ATTEST_UNKNOWN)),
                receipt: entry
                    .map(|entry| entry.receipt.clone())
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| String::from(NONE)),
                // Filled by [`attach_processes`], which asks the host a second
                // question. Left empty here so the version comparison — the
                // answer this command exists for — never depends on a process
                // lookup having succeeded.
                running_binary: None,
                binary_matches_process: None,
                verdict,
                detail,
            }
        })
        .collect()
}

/// Ask the host which artefact the live process under each named unit is
/// executing, and fill the two process fields of every row it answers for.
///
/// A second read on the same channel rather than two more fields on the version
/// reporter, because they are two different questions: the reporter answers what
/// is INSTALLED, this answers what is RUNNING, and the incidents that motivate
/// this column are precisely the cases where those two disagree while every
/// other column is correct.
///
/// One round trip per distinct unit, and only for units a row actually names: a
/// declared binary no unit runs has no process to ask about. A lookup that fails
/// leaves both fields `None` and nothing else changes — refusing to print the
/// version comparison because a secondary read failed would trade this
/// command's whole purpose against an addition to it.
pub(super) async fn attach_processes(target: &ComputeTarget, rows: &mut [Row], runner: &Runner) {
    let declared = service::declared_services(target);
    let mut asked: BTreeMap<String, Option<service::RunningProgram>> = BTreeMap::new();
    for row in rows.iter_mut() {
        if row.unit.is_empty() || row.unit == NONE {
            continue;
        }
        if !asked.contains_key(&row.unit) {
            // A unit the reporter named and the registry does not declare is
            // not asked about at all: locating its unit file would mean
            // guessing a path for a unit nobody adopted, which is the one
            // thing `service adopt` exists to stop.
            let found = declared
                .iter()
                .find(|candidate| candidate.matches(&row.unit));
            let program = match found {
                Some(service) => service::inspect_process(target, service, runner).await.ok(),
                None => None,
            };
            asked.insert(row.unit.clone(), program);
        }
        if let Some(Some(program)) = asked.get(&row.unit) {
            row.running_binary = program.running_binary().map(str::to_string);
            row.binary_matches_process = program.matches_process();
        }
    }
}
