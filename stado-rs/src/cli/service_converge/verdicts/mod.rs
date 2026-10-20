//! The verdict two versions and two revisions imply, and the process columns
//! attached to it.

pub(in crate::cli::service_converge) mod ordering;
pub(in crate::cli::service_converge) mod reporting;

use std::cmp::Ordering;
use std::collections::BTreeMap;

use crate::deploy::service;
use crate::deploy::Runner;
use crate::targets::ComputeTarget;

use crate::cli::service_converge::model::vocabulary::{
    Installed, Row, ATTEST_ABSENT, ATTEST_DIFFERS, ATTEST_NEVER_DELIVERED, ATTEST_UNKNOWN,
    HOST_AHEAD, HOST_BEHIND, HOST_MISSING, IN_SYNC, NONE, REVISION_MISMATCHED, UNATTESTED, UNKNOWN,
    VERSION_HELPER,
};
use crate::cli::service_converge::verdicts::ordering::version_order;

/// A reported text column as the table prints it: the word, or `absent` for
/// a column the host did not fill.
fn cell(value: Option<&str>, absent: &str) -> String {
    match value {
        Some(value) if !value.is_empty() => value.to_string(),
        _ => absent.to_string(),
    }
}

/// The version a row names in a sentence, or `unknown`.
fn named(installed: Option<&String>) -> &str {
    match installed {
        Some(version) => version.as_str(),
        None => UNKNOWN,
    }
}

/// The in-sync row's verdict once the version matches: whether the build is
/// the declared release's, read from the revision the host's delivery
/// receipt names against the revision the declared release's signed
/// manifest names.
fn same_version(
    entry: Option<&Installed>,
    declared_version: &str,
    declared_revision: &Result<String, String>,
) -> (&'static str, String) {
    let installed_revision = match entry {
        Some(entry) => entry.revision.as_deref(),
        None => None,
    };
    // Provenance, when the host kept a record of it. "-" still means
    // attested-by-bytes with no receipt, which is every delivery made before
    // the receipt format.
    let receipt = cell(entry.map(|entry| entry.receipt.as_str()), "-");
    match (installed_revision, declared_revision) {
        (Some(installed), Ok(declared)) if !installed.eq_ignore_ascii_case(declared) => (
            REVISION_MISMATCHED,
            format!(
                "the host runs {declared_version} built from {installed}, and the declared \
                 {declared_version} release was built from {declared}: the same version \
                 string, other code; `stado release version converge` delivers the declared \
                 release"
            ),
        ),
        (Some(_), Ok(_)) => (IN_SYNC, receipt),
        (None, Ok(declared)) => (
            IN_SYNC,
            format!(
                "{receipt}; its delivery receipt names no revision, so the build was not \
                 compared with the declared release's {declared}"
            ),
        ),
        (_, Err(cause)) => (
            IN_SYNC,
            format!(
                "{receipt}; the revision the declared release was built from could not be \
                 read, so the build was not compared: {cause}"
            ),
        ),
    }
}

/// One row per declared binary, each carrying the verdict its versions and
/// revisions imply. `revisions` holds, per binary, the revision its declared
/// release was built from, or why that could not be read.
pub(super) fn verdict_rows(
    declared: &[(String, String)],
    revisions: &BTreeMap<String, Result<String, String>>,
    reported: &Result<BTreeMap<String, Installed>, String>,
) -> Vec<Row> {
    declared
        .iter()
        .map(|(binary, declared_version)| {
            // A failed report has no entry for any binary; its failure is the
            // verdict's detail below, once for the host.
            let entry = reported.iter().find_map(|reported| reported.get(binary));
            let declared_revision = match revisions.get(binary) {
                Some(revision) => revision.clone(),
                None => Err(format!(
                    "no revision was asked for {binary} {declared_version}"
                )),
            };
            let installed = entry.and_then(|entry| entry.version.clone());
            let attestation = cell(
                entry.map(|entry| entry.attestation.as_str()),
                ATTEST_UNKNOWN,
            );
            let shown = named(installed.as_ref());
            // Provenance is judged before drift, because drift between a
            // declaration and bytes nobody delivered is not the finding: the
            // bytes are. Reading these as `host-ahead` is what let a local
            // build offer to promote its own version into the registry.
            let unattested = match attestation.as_str() {
                ATTEST_ABSENT => Some(format!(
                    "the host runs {shown} and no delivered copy of {shown} is staged at \
                     $HOME/.stado/releases, though earlier versions of this binary were \
                     delivered here; these bytes were put at the install path beside the \
                     delivery path, and converge will not move the declaration to a version \
                     it cannot attest"
                )),
                ATTEST_NEVER_DELIVERED => Some(format!(
                    "the host runs {shown} and this binary has never been delivered here: \
                     $HOME/.stado/releases holds no version of it at all. The bootstrap \
                     installer stages nothing, so this is the expected reading for a host \
                     that has not had a release delivery yet — it is not evidence that \
                     anything was replaced"
                )),
                ATTEST_DIFFERS => Some(format!(
                    "the host runs {shown} and the staged copy of {shown} does not match the \
                     installed file byte for byte; the binary was replaced after delivery"
                )),
                _ => None,
            };
            let (verdict, detail) = match (unattested, &installed, reported) {
                (Some(detail), _, _) => (UNATTESTED, detail),
                (None, Some(version), _) if version == declared_version => {
                    same_version(entry, declared_version, &declared_revision)
                }
                (None, Some(version), _) => match version_order(version, declared_version) {
                    // The host is behind the declaration: `--apply` delivers
                    // the declared one, which is an upgrade here.
                    Some(Ordering::Less) => (
                        HOST_BEHIND,
                        format!(
                            "the host runs {version}, older than the declared \
                             {declared_version}; `stado release version converge` delivers \
                             the declared one"
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
                             host; converge refuses to downgrade it and names the \
                             `version declare` command that moves the declaration"
                        ),
                    ),
                    // Equal orderings of unequal strings cannot happen for two
                    // exact semantic versions; one that does is compared by
                    // its build like any matching version.
                    Some(Ordering::Equal) => {
                        same_version(entry, declared_version, &declared_revision)
                    }
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
                (None, None, Err(failure)) => (UNKNOWN, failure.clone()),
                // The reporter answered and found no artefact at all. Its own
                // verdict, not `unknown`: this measurement succeeded, and what
                // it measured is a host that declares a binary it does not
                // carry. `--apply` delivers it, because there is nothing here
                // to downgrade and no process running the declared binary to
                // interrupt.
                (None, None, Ok(_))
                    if entry.is_some_and(|entry| entry.root.is_empty() || entry.root == NONE) =>
                {
                    (
                        HOST_MISSING,
                        format!(
                            "{VERSION_HELPER} found no installed artefact for this \
                             binary on this host; `stado release version converge` delivers \
                             the declared version"
                        ),
                    )
                }
                (None, None, Ok(_)) => (
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
            Row {
                binary: binary.clone(),
                declared: declared_version.clone(),
                installed,
                root: cell(entry.map(|entry| entry.root.as_str()), NONE),
                unit: cell(entry.map(|entry| entry.unit.as_str()), NONE),
                state: cell(entry.map(|entry| entry.state.as_str()), NONE),
                attestation,
                receipt: cell(entry.map(|entry| entry.receipt.as_str()), NONE),
                installed_revision: entry.and_then(|entry| entry.revision.clone()),
                declared_revision,
                // Filled by [`attach_processes`], which asks the host a second
                // question. Left empty here so the version comparison — the
                // answer this command exists for — never depends on a process
                // lookup having succeeded.
                running_binary: None,
                binary_matches_process: None,
                process_error: None,
                verdict,
                detail,
            }
        })
        .collect()
}

/// Ask the host which artefact the live process under each named unit is
/// executing, and fill the two process fields of every row it answers for.
///
/// Installed and running artifacts are independent observations. A unit can
/// keep executing an older artifact after its installed version changes,
/// so process identity is read separately from the version reporter.
///
/// One round trip per distinct unit, and only for units a row actually names: a
/// declared binary no unit runs has no process to ask about. A lookup that fails
/// leaves both fields `None`, records its cause in `process_error`, and nothing
/// else changes — refusing to print the version comparison because a secondary
/// read failed would trade this command's whole purpose against an addition to
/// it, but an `unknown` process cell with no cause would hide why.
pub(super) async fn attach_processes(target: &ComputeTarget, rows: &mut [Row], runner: &Runner) {
    let declared = service::declared_services(target);
    let mut asked: BTreeMap<String, Option<Result<service::RunningProgram, String>>> =
        BTreeMap::new();
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
                Some(service) => Some(
                    service::inspect_process(target, service, runner)
                        .await
                        .map_err(|error| error.to_string()),
                ),
                None => None,
            };
            asked.insert(row.unit.clone(), program);
        }
        match asked.get(&row.unit) {
            Some(Some(Ok(program))) => {
                row.running_binary = program.running_binary().map(str::to_string);
                row.binary_matches_process = program.matches_process();
            }
            Some(Some(Err(cause))) => row.process_error = Some(cause.clone()),
            _ => {}
        }
    }
}
