//! `stado fleet needs` on a terminal: one block per need, plain sentences.

use super::advisor::NeedsReport;

/// The sentence for a fleet that wants nothing.
pub fn empty_sentence(window_days: i64) -> String {
    format!("the fleet reports no unmet need in the last {window_days} days")
}

pub fn render(report: &NeedsReport) -> String {
    if report.needs.is_empty() {
        return format!("{}\n", empty_sentence(report.window_days));
    }
    let mut out = String::new();
    for need in &report.needs {
        let subject = match (&need.target, &need.platform) {
            (Some(target), _) => target.clone(),
            (None, Some(platform)) => platform.clone(),
            (None, None) => "fleet".to_string(),
        };
        out.push_str(&format!(
            "{} {} ({}): {}\n",
            need.severity.as_str(),
            need.need.as_str(),
            subject,
            need.summary
        ));
        for evidence in &need.evidence {
            out.push_str(&format!("  {}: {}\n", evidence.source, evidence.detail));
        }
        out.push_str(&format!("  suggestion: {}\n", need.suggestion));
    }
    out
}
