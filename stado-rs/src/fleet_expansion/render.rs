use super::model::{need_key, ExpansionReport};
use std::fmt::Write;

fn value(value: Option<f64>) -> String {
    value
        .map(|v| format!("{v:.2}"))
        .unwrap_or_else(|| "unknown / no finite return".into())
}

pub fn render_report(report: &ExpansionReport) -> String {
    let mut out = format!(
        "expansion plan {}: {}\nbudget {:.2} USD; horizon {} months; evidence window {} days\n",
        report.plan_id, report.status, report.budget_usd, report.horizon_months, report.window_days
    );
    for need in &report.needs {
        let _ = writeln!(out, "need {}: {}", need_key(need), need.summary);
        for evidence in &need.evidence {
            let _ = writeln!(out, "  {}: {}", evidence.source, evidence.detail);
        }
    }
    for row in &report.candidates {
        let selected = report.portfolio.selected_ids.contains(&row.option.id);
        let _ = writeln!(
            out,
            "{} {}: {}{}",
            row.option.id,
            row.status,
            row.option.label,
            if selected { " [selected]" } else { "" }
        );
        let _ = writeln!(out, "  expenditure {} USD; monthly net {} USD; horizon gain {} USD; payback {} months; ROI {}%", value(row.committed_cost_usd), value(row.monthly_net_usd), value(row.horizon_net_usd), value(row.payback_months), value(row.roi_pct));
        for reason in &row.reasons {
            let _ = writeln!(out, "  excluded: {reason}");
        }
        let _ = writeln!(
            out,
            "  evidence: {} ({} through {})",
            row.option.evidence, row.option.observed_at, row.option.valid_until
        );
    }
    let p = &report.portfolio;
    let _ = writeln!(out, "selected: {}\nexpenditure {:.2} USD; budget remaining {:.2} USD; monthly net {:.2} USD; horizon gain {:.2} USD; payback {} months; ROI {}%", if p.selected_ids.is_empty() { "none".into() } else { p.selected_ids.join(", ") }, p.committed_cost_usd, p.remaining_budget_usd, p.monthly_net_usd, p.horizon_net_usd, value(p.payback_months), value(p.roi_pct));
    for warning in &report.warnings {
        let _ = writeln!(out, "warning: {warning}");
    }
    out
}
