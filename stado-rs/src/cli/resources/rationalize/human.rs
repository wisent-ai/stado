//! The operator-facing rendering: one row per recommendation, one row per
//! source, and a closing line that repeats that nothing was changed.

use crate::cli::reporting::table;
use crate::cli::resources::model::SourceCoverage;

use super::RationalizationReport;

/// What a source that was not read whole says about why, in the fields its
/// detail carries: the error it met, the reason it was not read, and what
/// would let it be read.
#[derive(serde::Deserialize)]
struct Cause {
    error: Option<String>,
    reason: Option<String>,
    remedy: Option<String>,
}

pub(super) fn print_human(report: &RationalizationReport) {
    let rows: Vec<Vec<String>> = report
        .findings
        .iter()
        .map(|finding| {
            vec![
                finding.severity.to_string().to_uppercase(),
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
                source_why(&source.state, &source.detail),
            ]
        })
        .collect();
    table::print(&["SOURCE", "STATE", "WHY"], &source_rows);
    println!(
        "\n{} recommendation(s); {} incomplete source(s); read-only, no changes applied.",
        report.summary.findings, report.summary.incomplete_sources
    );
}

/// Why a source is in its state: nothing for one read in full; otherwise its
/// error, reason and remedy as it gave them, and a source that gave none is
/// said to have reported none rather than shown blank.
fn source_why(state: &SourceCoverage, detail: &serde_json::Value) -> String {
    if matches!(state, SourceCoverage::Ok) {
        return String::new();
    }
    let said = match serde_json::from_value::<Cause>(detail.clone()) {
        Ok(cause) => {
            let remedy = cause.remedy.map(|remedy| format!("remedy: {remedy}"));
            [cause.error, cause.reason, remedy]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join("; ")
        }
        Err(error) => format!("the source's detail is not readable: {error}"),
    };
    if said.is_empty() {
        crate::cli::reporting::NOT_REPORTED.to_string()
    } else {
        said
    }
}
