//! The standing declared-against-running sweep, as one doctor row.

use std::time::Duration;

use crate::doctor::{Check, Findings, Status};

// ---------------------------------------------------------------------------
// 11. Fleet shape
// ---------------------------------------------------------------------------

pub(in crate::doctor) const SHAPE_ID: &str = "fleet-shape";
pub(in crate::doctor) const SHAPE_TITLE: &str = "Fleet shape: declared against running";
pub(in crate::doctor) const SHAPE_REMEDY: &str =
    "each finding names the command that resolves it; the same sweep runs on every coordinator \
     tick, so a finding here is not waiting on anyone typing this";

/// Per-host work times the fleet, so this row cannot share the flat probe
/// budget: it reads listeners, loaded units and disk from every managed host.
pub(in crate::doctor) const FLEET_SHAPE_DEADLINE: Duration = Duration::from_secs(600);

/// The standing checks in [`crate::fleet_shape`], as one doctor row.
///
/// A sweep that measured nothing is a FAIL and not a PASS. That distinction is
/// the entire reason this check exists: the fleet spent a night full of
/// declarations that nothing compared against reality, and a green row on an
/// unmeasured fleet would be one more of them.
pub(in crate::doctor) async fn check_fleet_shape() -> Check {
    let runner = crate::deploy::production_runner();
    let mut sweep = crate::fleet_shape::sweep(&runner).await;
    if let Some(finding) = crate::fleet_shape::health_disagreement().await {
        sweep.measured += 1;
        sweep.findings.push(finding);
    }
    let mut findings = Findings::default();
    // A host nobody could measure is a FAIL, not a WARN. It used to be a
    // warning beside a PASS summary, and the summary counted the hosts that
    // DID answer -- so `fleet-shape` could report `4 subject(s) measured` and
    // a green line while one host in the fleet had been asked nothing at all.
    // Every check in this module exists because something reported clean
    // without looking, and a warning that renders under a pass is that same
    // shape wearing the instrument's own badge.
    for (host, reason) in &sweep.unreachable {
        findings.note(
            Status::Fail,
            format!(
                "every-host-is-measured: {host} — every managed host is swept — \
                 observed not measured: {reason}"
            ),
        );
        findings.remedy(format!("stado host link {host}"));
    }
    for finding in &sweep.findings {
        findings.note(Status::Fail, finding.line());
        findings.remedy(finding.command.clone());
    }
    // The per-rule counts the sweep already computed, as fields beside the
    // prose. Every host swept contributes one row per rule, so a rule that
    // proved nothing on one host is visible without reading a sentence.
    for measurement in &sweep.measurements {
        findings.measure(measurement.clone());
    }
    findings.measure(crate::fleet_shape::Measurement::new(
        SHAPE_ID,
        None,
        u64::from(sweep.measured),
    ));
    if sweep.measured == 0 {
        findings.note(Status::Fail, sweep.summary());
    } else {
        findings.note(Status::Pass, sweep.summary());
    }
    findings.into_check(SHAPE_ID, SHAPE_TITLE, SHAPE_REMEDY)
}
