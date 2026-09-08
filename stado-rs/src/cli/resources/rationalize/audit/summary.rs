//! The audit summary: how many recommendations there are, how they are
//! distributed, and whether the sources behind them were complete. An
//! incomplete source keeps the state honest even when nothing was found.

use std::collections::BTreeMap;

use crate::cli::resources::rationalize::{Finding, Summary};

pub(super) fn summarize(findings: &[Finding], incomplete_sources: usize) -> Summary {
    let mut by_severity = BTreeMap::new();
    let mut by_action = BTreeMap::new();
    for finding in findings {
        *by_severity
            .entry(finding.severity.to_string())
            .or_insert(usize::default()) += 1;
        *by_action
            .entry(finding.action.to_string())
            .or_insert(usize::default()) += 1;
    }
    let state = match (findings.is_empty(), incomplete_sources == usize::default()) {
        (true, true) => "clean",
        (false, true) => "recommendations",
        (true, false) => "incomplete",
        (false, false) => "incomplete_with_recommendations",
    };
    Summary {
        state,
        findings: findings.len(),
        incomplete_sources,
        by_severity,
        by_action,
    }
}

pub(super) fn severity_rank(severity: &str) -> u8 {
    match severity {
        "high" => u8::default(),
        "medium" => u8::from(true),
        "low" => u8::from(true).saturating_add(u8::from(true)),
        _ => u8::MAX,
    }
}
