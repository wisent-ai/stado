//! The operator's table for `stado host provenance`: one row per artifact a
//! host carries, the reasons a row could not be completed, and the sentences
//! under the table.

use super::trailers::Counts;
use super::CarriedArtifact;

/// What the report found, as the table and its trailers say it.
pub(super) struct Findings<'a> {
    pub(super) target: &'a str,
    pub(super) carried: &'a [CarriedArtifact],
    pub(super) counts: &'a Counts,
    /// Manifests, receipts and artifact lines that could not be read.
    pub(super) unreadable: &'a [String],
    /// Why reachability could not be resolved where a check was attempted.
    pub(super) unresolved_causes: &'a [String],
    pub(super) repository_missing: bool,
}

/// Print the table, after every reason a row is incomplete.
pub(super) fn print(findings: &Findings<'_>) {
    let target = findings.target;
    let rows: Vec<Vec<String>> = findings
        .carried
        .iter()
        .map(|item| {
            let age = match (item.age_seconds, item.accounted()) {
                (Some(seconds), _) => {
                    crate::cli::registry::human_age(chrono::TimeDelta::seconds(seconds))
                }
                // A record whose timestamp will not parse was hand-edited; say
                // so instead of showing an age.
                (None, true) => "unreadable stamp".to_string(),
                (None, false) => "never".to_string(),
            };
            let reachable = match item.reachable {
                Some(true) => "yes",
                Some(false) => "no",
                None => "not checked",
            };
            let describes = match item.describes {
                Some(true) => "match",
                Some(false) => "REPLACED",
                None => "-",
            };
            vec![
                item.artifact.clone(),
                item.accounted_by().as_str().to_string(),
                item.commit(),
                item.builder(),
                age,
                reachable.to_string(),
                describes.to_string(),
            ]
        })
        .collect();
    // Before the early return as well as before the table: a host whose only
    // provenance file is corrupt must not read as a host with nothing to say.
    for error in findings.unreadable {
        eprintln!("{target}: a provenance manifest could not be read: {error}");
    }
    for cause in findings.unresolved_causes {
        eprintln!("{target}: reachability is unresolved: {cause}");
    }
    if rows.is_empty() {
        println!("{target}: carries no stado-managed programs");
        return;
    }
    crate::cli::reporting::table::print(
        &[
            "ARTIFACT",
            "ACCOUNTED",
            "COMMIT",
            "BUILDER",
            "AGE",
            "REACHABLE",
            "BYTES",
        ],
        &rows,
    );
    super::trailers::print(
        target,
        findings.carried,
        findings.counts,
        rows.len(),
        findings.repository_missing,
    );
}
