//! The operator-facing rendering: one row per recommendation, one row per
//! source, and a closing line that repeats that nothing was changed.

use serde_json::Value;

use crate::cli::table;

use super::RationalizationReport;

pub(super) fn print_human(report: &RationalizationReport) {
    let rows: Vec<Vec<String>> = report
        .findings
        .iter()
        .map(|finding| {
            vec![
                finding.severity.to_uppercase(),
                finding.action.to_string(),
                finding.confidence.to_string(),
                finding.provider.clone(),
                finding.resource_type.to_string(),
                finding.resource.clone(),
                finding.reason.clone(),
            ]
        })
        .collect();
    table::print(
        &[
            "SEVERITY",
            "ACTION",
            "CONFIDENCE",
            "PROVIDER",
            "TYPE",
            "RESOURCE",
            "WHY",
        ],
        &rows,
    );

    let source_rows: Vec<Vec<String>> = report
        .sources
        .iter()
        .map(|source| {
            vec![
                source.name.clone(),
                source.state.to_string(),
                source
                    .detail
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            ]
        })
        .collect();
    table::print(&["SOURCE", "STATE", "ERROR"], &source_rows);
    println!(
        "\n{} recommendation(s); {} incomplete source(s); read-only, no changes applied.",
        report.summary.findings, report.summary.incomplete_sources
    );
}
