//! One cause, one row: the rows that are only a reported cause's symptom.

use crate::cli::registry::doctor::findings::Finding;

/// Drop every `missing-plist` row whose cause is already reported for the
/// same target and unit.
pub(in crate::cli::registry::doctor) fn prune_symptoms(findings: &mut Vec<Finding>) {
    // One cause, one row. A unit the beacon does not report on a host that cannot
    // satisfy what the unit needs is already reported as `capability-unsatisfied`,
    // and the `missing-plist` row for the same label is that finding's symptom:
    // installing the plist would not make the host able to run it. A unit
    // declared in a domain its host cannot have is the same relationship —
    // `misdeclared-domain` is why nothing loads it and why no beacon reports
    // it, and installing the plist where it is declared would change neither.
    let caused: Vec<(String, String)> = findings
        .iter()
        .filter(|finding| {
            matches!(
                finding.kind,
                "capability-unsatisfied" | "misdeclared-domain"
            )
        })
        .filter_map(|finding| {
            finding
                .unit
                .clone()
                .map(|unit| (finding.subject.clone(), unit))
        })
        .collect();
    findings.retain(|finding| {
        if finding.kind != "missing-plist" {
            return true;
        }
        let Some(unit) = finding.unit.as_deref() else {
            return true;
        };
        !caused
            .iter()
            .any(|(subject, symptom)| subject == &finding.subject && symptom == unit)
    });
}
