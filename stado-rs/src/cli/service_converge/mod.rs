//! `stado service converge` — is the host running the version the registry
//! declares for it, and if not, put it there.
//!
//! Every other command in this group answers a question about a *unit*: is it
//! loaded, what does it run, what is in its environment, when did it last
//! restart. Not one of them could answer the question that actually cost this
//! fleet a day: **is the program on that host the build we shipped?** A
//! declaration named a label and a plist path, both of which stayed true across
//! every release that never reached the box, so a mac mini serving an old
//! version was byte-for-byte indistinguishable from one at the declared one —
//! `service list` said `active`, `service show` printed the same program path
//! it always had, and the beacons agreed. Nothing was wrong with any of those
//! answers. None of them was about the code.
//!
//! The primitive this compares is the one the fleet already delivers against:
//! `targets[].managed_versions`, the registry's per-binary statement of the
//! exact version a host must run. Not a git commit — the hosts do not carry
//! checkouts. `control-host` runs Weles as an installed release artefact
//! with a `package.json`, a `.weles-release` stamp and a `provenance.json`
//! beside it and no `.git` anywhere, and a converge that compared commits
//! there could only ever report "unknown" about a product that is in fact
//! precisely versioned.
//!
//! Four verdicts, never two:
//!
//!   unattested  the host runs bytes whose provenance cannot be shown: the
//!               version they claim has no delivered copy staged on the host,
//!               or the installed file is not that copy. Judged BEFORE drift,
//!               because a version string is not provenance and reading a
//!               local build as `host-ahead` is how it offers to write its own
//!               version into the registry.
//!   in-sync     the host runs exactly the declared version.
//!   host-behind the host runs a version strictly OLDER than the declared
//!               one. This is the state that hid behind a passing
//!               `service list` for as long as it took somebody to notice
//!               the behaviour was old. `--apply` delivers the declared
//!               version through `stado release host-state --host TARGET --apply`.
//!   host-ahead  the host runs a version strictly NEWER than the declared one:
//!               the declaration is stale, and delivering it would DOWNGRADE a
//!               live host. `--apply` refuses and names the `stado release
//!               declare-version` command that moves the declaration.
//!   unknown     the host said nothing usable: the reporter could not run, the
//!               channel refused, or the artefact carries no
//!               version metadata at all. Kept apart from both drift verdicts
//!               for the same
//!               reason [`crate::cli::service_verify`] keeps `unverified` apart
//!             from `unreachable` — "I did not look" and "I looked and it is
//!             wrong" send an operator to two different places, and folding
//!             them together is how a fleet learns to ignore its own reports.
//!             It is never folded into `in-sync` either: an unmeasurable
//!             product is reported as unmeasured, in its own row, every time.
//!
//! The exit codes follow from that split, and the split is the whole reason
//! they differ:
//!
//! - **report mode** exits non-zero on `host-behind` or `host-ahead` alone.
//!   Either is a false declaration and a gate should fail on it; an
//!   uninstalled reporter is
//!   not evidence of anything and must not masquerade as drift, exactly as
//!   `service verify` refuses to let a missing probe masquerade as an outage.
//!   Every `unknown` row is still named on stderr, so nothing about it is
//!   silent.
//! - **`--apply`** exits non-zero unless every binary in scope came back
//!   `in-sync`. An operator who asked for convergence is owed proof of it, and
//!   "the reporter is not installed" is not proof — after an apply, an
//!   unconfirmed binary is a failed apply.
//!
//! Two things this command deliberately does not do. It never writes the
//! registry: the declared version is the operator's statement of intent,
//! published through `stado release declare-version`, and a convergence that
//! edited the document to match the host would turn a drift report into a
//! rubber stamp. Closing the gap is
//! [`crate::deploy::host_release::release_host`], called in-process by
//! `stado release host-state --host TARGET --apply`. One fetch, one digest
//! check, one staging tree, one `rename(2)`, one restart — there is one path
//! that both reports drift and delivers the declaration.

use crate::deploy::{host_channel, production_runner, DeployError};

use super::CmdError;

mod converging;
mod model;
mod observing;
mod verdicts;

pub use crate::cli::service_converge::model::vocabulary::{
    HOST_AHEAD, HOST_BEHIND, IN_SYNC, UNATTESTED, UNDECLARED, UNKNOWN,
};
pub use crate::cli::service_converge::model::ServiceConvergeResult;

use crate::cli::service_converge::converging::apply_releases;
use crate::cli::service_converge::converging::readers::converge_native_readers;
use crate::cli::service_converge::model::receipts::FAILED;
use crate::cli::service_converge::model::vocabulary::ATTEST_MATCH;
use crate::cli::service_converge::observing::declaration::declaring;
use crate::cli::service_converge::observing::read_installed;
use crate::cli::service_converge::verdicts::attach_processes;
use crate::cli::service_converge::verdicts::reporting::emit;
use crate::cli::service_converge::verdicts::reporting::gates::{
    apply_gate_diagnostics, report_gate_diagnostics,
};
use crate::cli::service_converge::verdicts::verdict_rows;

fn click(error: DeployError) -> CmdError {
    CmdError::click(error.to_string())
}

/// Execute one report or apply operation and retain its complete structured
/// result.
///
/// Failures returned from here happen before a report exists: target
/// resolution and declaration validation both finish before the host is
/// contacted. Once observation begins, channel and delivery failures are
/// findings in the report, and the result carries the command's real non-zero
/// gate rather than turning that completed operation into a transport error.
pub async fn converge_result(
    target: &str,
    binary: Option<&str>,
    apply: bool,
) -> Result<ServiceConvergeResult, CmdError> {
    let resolved = host_channel::canonical_target(target)
        .await
        .map_err(click)?;
    let declared = declaring(&resolved, binary)?;
    if declared.is_empty() {
        return Ok(ServiceConvergeResult::new(resolved.name, None, Vec::new()));
    }
    crate::deploy::products::managed_platform(resolved.release_platform.trim()).map_err(
        |error| {
            CmdError::click(format!(
                "{} declares release_platform {:?}, which cannot carry a managed release: {}; \
             set targets[].release_platform to a published platform",
                resolved.name, resolved.release_platform, error
            ))
        },
    )?;
    let runner = production_runner();

    let reported = read_installed(&resolved, &runner).await;
    let mut rows = verdict_rows(&declared, &reported);
    attach_processes(&resolved, &mut rows, &runner).await;
    if !apply {
        return Ok(ServiceConvergeResult::new(resolved.name, None, rows));
    }

    let mut pass = apply_releases(&resolved.name, &rows, &runner).await;
    // Re-read rather than trust delivery's own word. A delivery that reports
    // `released` has testified about its own work, which is the
    // one witness that cannot establish the fact being claimed; the version the
    // host reports afterwards comes back through the same reporter that
    // produced the drift finding, so a successful delivery and a confirmed
    // convergence are not the same claim.
    let reported = read_installed(&resolved, &runner).await;
    let mut rows = verdict_rows(&declared, &reported);
    let stado_root_in_sync = rows.iter().any(|row| {
        row.binary == "stado"
            && row.verdict == IN_SYNC
            && reported
                .as_ref()
                .ok()
                .and_then(|entries| entries.get("stado"))
                .is_some_and(|entry| entry.attestation == ATTEST_MATCH)
    });
    let root_delivery_failed = pass
        .releases
        .iter()
        .any(|release| release.binary == "stado" && release.status == FAILED);
    if stado_root_in_sync && !root_delivery_failed {
        converge_native_readers(&resolved, &declared, &runner, &mut pass).await;
    }
    // Asked again after the delivery for the same reason the versions are: a
    // release ends in a restart, and whether the restarted process is executing
    // the artefact that was just installed is exactly the claim `--apply` is
    // being asked to prove.
    attach_processes(&resolved, &mut rows, &runner).await;
    Ok(ServiceConvergeResult::new(resolved.name, Some(pass), rows))
}

/// `stado service converge TARGET [BINARY] [--apply]`.
pub async fn converge(
    target: &str,
    binary: Option<&str>,
    apply: bool,
    json_output: bool,
) -> Result<(), CmdError> {
    let result = converge_result(target, binary, apply).await?;
    emit(&result, json_output)?;
    match result.applied.as_ref() {
        Some(pass) => apply_gate_diagnostics(&result.rows, pass, result.exit_code),
        None => report_gate_diagnostics(&result.rows, result.exit_code),
    }
    if result.exit_code == i32::default() {
        Ok(())
    } else {
        Err(CmdError::silent(result.exit_code))
    }
}
