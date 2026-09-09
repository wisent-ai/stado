//! Converging: deliver the declared version of every binary behind it, and
//! refuse only what a delivery would take backwards.

pub(in crate::cli::service_converge) mod readers;

use std::cmp::Ordering;

use serde_json::Value;

use crate::deploy::{host_release, Runner};

use crate::cli::service_converge::model::receipts::{
    AppliedPass, Refused, Released, Undeliverable, COMPLETED, FAILED,
};
use crate::cli::service_converge::model::vocabulary::{
    Row, HOST_AHEAD, HOST_BEHIND, HOST_MISSING, UNATTESTED,
};
use crate::cli::service_converge::verdicts::ordering::version_order;

// ---------------------------------------------------------------------------
// Converging
// ---------------------------------------------------------------------------

/// Deliver the declared version of every binary that is behind it or running
/// bytes the fleet cannot attest, and refuse only what a delivery would take
/// backwards.
///
/// This is the delivery owned by `stado release host-state --apply`, called
/// in-process rather than reimplemented: the digest check against the canonical
/// release manifest, versioned staging, activation, and unit restart all happen
/// exactly once.
///
/// A binary the registry declares but no product declaration carries is
/// recorded as undeliverable and never attempted: that refusal is made
/// against the shipped product declaration
/// ([`crate::deploy::products`]), so asking the host about it would cost an
/// ssh connection to learn something already known here.
///
/// `unknown` rows are deliberately not delivered. Nothing is known to be wrong
/// with them, delivery ends in a unit restart, and restarting a working service
/// on the strength of a reporter that failed to answer is how a healthy host
/// goes down because a report was missing. `host-missing` is the other half of
/// that sentence and is delivered: there the reporter answered and the answer
/// was that the host holds no copy of the binary it declares.
///
/// `host-ahead` rows are refused outright: the host runs NEWER than the
/// declaration, so delivering the declared version is a downgrade of a live
/// host, and a converge that performs one is the registry's staleness shipped
/// as an outage. Each refusal records the exact `stado release declare-version`
/// command that moves the declaration to the observed version instead.
pub(super) async fn apply_releases(target: &str, rows: &[Row], runner: &Runner) -> AppliedPass {
    let mut pass = AppliedPass::default();
    // An unattested binary is delivered, not reported at.
    //
    // Earlier versions refused every unattested row even though delivery is
    // exactly what replaces bytes the fleet cannot attest. This path now owns
    // that delivery. Only an unattested binary strictly ahead of its
    // declaration remains refused, because replacing it would be a downgrade.
    //
    // The one case still refused is a host strictly AHEAD of its declaration:
    // there a delivery takes a live host backwards on a stale declaration, and
    // the remediation stays a delivery rather than `declare-version`, because
    // writing an unattested version into the registry is the failure, not the
    // fix.
    for row in rows.iter().filter(|row| row.verdict == UNATTESTED) {
        let ahead = row
            .installed
            .as_deref()
            .and_then(|installed| version_order(installed, &row.declared))
            == Some(Ordering::Greater);
        if ahead {
            pass.refused.push(Refused {
                binary: row.binary.clone(),
                declared: row.declared.clone(),
                installed: row.installed_cell().to_string(),
                remediation: format!(
                    "stado release host-state --host {target} --binary {} --apply \
                     (deliver a published version; do not declare unattested bytes)",
                    row.binary
                ),
            });
            continue;
        }
        eprintln!(
            "{}: runs {}, which this fleet cannot attest: {}",
            row.binary,
            row.installed_cell(),
            row.detail
        );
        deliver(target, row, runner, &mut pass).await;
    }
    for row in rows.iter().filter(|row| row.verdict == HOST_AHEAD) {
        let remediation = format!(
            "stado release declare-version --host {target} --binary {} --version {}",
            row.binary,
            row.installed_cell()
        );
        pass.refused.push(Refused {
            binary: row.binary.clone(),
            declared: row.declared.clone(),
            installed: row.installed_cell().to_string(),
            remediation,
        });
    }
    for row in rows.iter().filter(|row| row.verdict == HOST_BEHIND) {
        eprintln!(
            "{}: declared {} but runs {}",
            row.binary,
            row.declared,
            row.installed_cell()
        );
        deliver(target, row, runner, &mut pass).await;
    }
    // A host that declares a binary and carries none. The first install is a
    // delivery like any other: nothing is replaced, no process is running the
    // declared binary, and the alternative - what this command did until
    // 2026-09-08 - is that a host with no copy could never be given one by
    // the product at all, so the first copy arrived by hand.
    for row in rows.iter().filter(|row| row.verdict == HOST_MISSING) {
        eprintln!(
            "{}: declared {} and this host carries no copy of it",
            row.binary, row.declared
        );
        deliver(target, row, runner, &mut pass).await;
    }
    pass
}

/// One delivery, recorded. The only call site of
/// [`host_release::release_host`] in this command, so a host that is behind and
/// a host whose bytes cannot be attested are converged by the same path and
/// cannot drift apart in what "delivered" means.
async fn deliver(target: &str, row: &Row, runner: &Runner, pass: &mut AppliedPass) {
    if let Err(error) = crate::deploy::products::product(&row.binary) {
        pass.undeliverable.push(Undeliverable {
            binary: row.binary.clone(),
            detail: error.to_string(),
        });
        return;
    }
    eprintln!("{target}: releasing {} {}", row.binary, row.declared);
    match host_release::release_host(target, &row.binary, &row.declared, false, false, runner).await
    {
        Ok(report) => {
            let status = report
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let delivered = matches!(
                status,
                host_release::RELEASED_STATUS | host_release::ALREADY_ACTIVE_STATUS
            );
            // Delivery reports host-side refusals as a structured `Ok(report)`,
            // with any diagnostic in `error`. Reducing that
            // report to its status discarded the only place a cause could be
            // retained: historical trains printed only `detail: "failed"`,
            // which does not establish whether the inner report had an error.
            let detail = if delivered {
                status.to_string()
            } else {
                report
                    .get("error")
                    .and_then(Value::as_str)
                    .filter(|detail| !detail.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| {
                        if status.is_empty() {
                            String::from("the delivery reported neither a status nor an error")
                        } else {
                            format!(
                                "delivery returned non-success status {status} without an error"
                            )
                        }
                    })
            };
            pass.releases.push(Released {
                binary: row.binary.clone(),
                version: row.declared.clone(),
                status: if delivered { COMPLETED } else { FAILED },
                detail,
            });
        }
        Err(error) => pass.releases.push(Released {
            binary: row.binary.clone(),
            version: row.declared.clone(),
            status: FAILED,
            detail: error.to_string(),
        }),
    }
}
