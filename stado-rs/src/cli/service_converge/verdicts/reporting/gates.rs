//! The two gates: which verdicts fail a report, and which fail an apply.

use crate::cli::service_converge::model::receipts::{AppliedPass, FAILED};
use crate::cli::service_converge::model::vocabulary::{
    Row, HOST_AHEAD, HOST_BEHIND, IN_SYNC, UNATTESTED, UNKNOWN,
};
use crate::cli::CLICK_ERROR_CODE;

/// Report mode: drift in either direction fails, an unmeasured binary does
/// not.
///
/// This is what makes the command usable as a gate. A host behind or ahead of
/// its declaration is a false declaration and belongs in a non-zero exit; a
/// host whose reporter is not installed, or a product whose artefact carries
/// no version metadata, has produced no evidence either way, and turning that
/// into a failure teaches operators to pass `|| true`, at which point the
/// drift the command exists to catch stops being noticed again. Every such
/// row is named on stderr instead, because the one thing an unmeasured
/// product must never be is quiet.
pub(in crate::cli::service_converge) fn report_exit_code(rows: &[Row]) -> i32 {
    if rows.iter().any(|row| {
        row.verdict == HOST_BEHIND || row.verdict == HOST_AHEAD || row.verdict == UNATTESTED
    }) {
        CLICK_ERROR_CODE
    } else {
        i32::default()
    }
}

pub(in crate::cli::service_converge) fn report_gate_diagnostics(rows: &[Row], exit_code: i32) {
    for row in rows.iter().filter(|row| row.verdict == UNKNOWN) {
        eprintln!(
            "{}: declared {} and no installed version could be read — unmeasured, \
             not in sync: {}",
            row.binary, row.declared, row.detail
        );
    }
    if exit_code == i32::default() {
        return;
    }
    let behind = rows.iter().filter(|row| row.verdict == HOST_BEHIND).count();
    let ahead = rows.iter().filter(|row| row.verdict == HOST_AHEAD).count();
    let unattested = rows.iter().filter(|row| row.verdict == UNATTESTED).count();
    // Named first and loudest: a version the fleet cannot attest outranks a
    // version it can attest and disagrees with.
    if unattested != 0 {
        eprintln!(
            "{unattested} declared binary/binaries run bytes this fleet cannot attest: \
             the version they claim has no delivered copy staged on the host, or the \
             installed file is not the one that was staged. A version string is not \
             provenance. Deliver with `stado release host-state --apply`; do not move \
             the declaration onto them"
        );
    }
    if behind != 0 {
        eprintln!(
            "{behind} declared binary/binaries run a version older than the \
             registry declares; re-run with --apply to deliver the declared one"
        );
    }
    if ahead != 0 {
        eprintln!(
            "{ahead} declared binary/binaries run a version NEWER than the \
             registry declares: the declaration is stale, not the host; \
             `stado release declare-version` moves it, --apply will not touch these hosts"
        );
    }
}

/// Apply mode: anything short of `in-sync` is a failed apply.
///
/// The operator asked for the host to be brought to the declared version, so
/// the only acceptable end state is one this command has confirmed by reading
/// the host again. `unknown` counts as failure here and does not in report
/// mode, and that is the intended difference: before an apply it means nobody
/// looked, after one it means the convergence cannot be shown to have happened.
pub(in crate::cli::service_converge) fn apply_exit_code(rows: &[Row], pass: &AppliedPass) -> i32 {
    if rows.iter().all(|row| row.verdict == IN_SYNC)
        && pass.releases.iter().all(|entry| entry.status != FAILED)
        && pass.undeliverable.is_empty()
        && pass.refused.is_empty()
    {
        i32::default()
    } else {
        CLICK_ERROR_CODE
    }
}

pub(in crate::cli::service_converge) fn apply_gate_diagnostics(
    rows: &[Row],
    pass: &AppliedPass,
    exit_code: i32,
) {
    if exit_code == i32::default() {
        return;
    }
    let unresolved: Vec<&Row> = rows.iter().filter(|row| row.verdict != IN_SYNC).collect();
    let failed = pass
        .releases
        .iter()
        .filter(|entry| entry.status == FAILED)
        .count();
    for row in &unresolved {
        eprintln!(
            "{}: declared {} != installed {}",
            row.binary,
            row.declared,
            row.installed_cell()
        );
    }
    for entry in &pass.refused {
        eprintln!(
            "{}: runs {}, newer than the declared {} — refused to downgrade the \
             host; move the declaration instead: {}",
            entry.binary, entry.installed, entry.declared, entry.remediation
        );
    }
    // A failed delivery remains a failed apply even when the final read finds
    // matching bytes (for example because another release actor converged the
    // host concurrently). The delivery receipt is an asserted part of this
    // operation, not disposable progress text.
    // "no delivery ran" is a different diagnosis from "one ran and failed", and
    // both are different from "one ran, said it worked, and the host still
    // reports the old version" — and different again from "the drift is real
    // and nothing in this pack delivers that binary". The summary line names
    // which of the four this was, because the next action an operator takes
    // differs for every one of them.
    let mut effort = match (pass.releases.len(), failed) {
        (0, _) => String::from("no delivery ran"),
        (total, 0) => format!("{total} delivery/deliveries, none of which failed"),
        (total, failed) => format!("{total} delivery/deliveries, {failed} of which failed"),
    };
    if pass.undeliverable.is_empty() {
        if pass.releases.is_empty() && pass.refused.is_empty() {
            effort.push_str(", because nothing was confirmed behind its declaration");
        }
    } else {
        effort.push_str(&format!(
            "; {} host-behind binary/binaries have no declared release product",
            pass.undeliverable.len()
        ));
    }
    // Every refusal used to be reported as `host-ahead`, whatever it was. On
    // 2026-09-02 charless-mac-mini was BEHIND its declaration — 0.13.45 against
    // 0.13.46 — and the summary told the release train that a host ahead of the
    // registry had been refused rather than downgraded, which is the opposite
    // diagnosis and points an operator at `declare-version` when the answer was
    // a delivery. A refusal is classified by the row it came from.
    if !pass.refused.is_empty() {
        let kind = |verdict: &str| {
            pass.refused
                .iter()
                .filter(|entry| {
                    rows.iter()
                        .any(|row| row.binary == entry.binary && row.verdict == verdict)
                })
                .count()
        };
        let ahead = kind(HOST_AHEAD);
        let unattested = kind(UNATTESTED);
        if ahead != 0 {
            effort.push_str(&format!(
                "; {ahead} host-ahead binary/binaries were refused rather than downgraded — \
                 the declaration is stale, not the host"
            ));
        }
        if unattested != 0 {
            effort.push_str(&format!(
                "; {unattested} binary/binaries run unattested bytes NEWER than the \
                 declaration, so no delivery was made — move the declaration to a published \
                 version first"
            ));
        }
    }
    eprintln!(
        "{} binary/binaries are not at their declared version after {effort}",
        unresolved.len()
    );
}
