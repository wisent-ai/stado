//! `stado resources rationalize` — read-only audit and immutable plan creation.
//!
//! The report vocabulary lives here because every component of the command
//! names it: `audit` collects findings from the Stado configuration, the
//! authoritative queue and the provider inventories, `plan` compiles the
//! selected findings into an immutable plan of reversible and irreversible
//! actions, and `human` prints the operator-facing tables.

mod audit;
mod human;
mod plan;

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{json, Value};

use super::journal::Journal;
use super::model::Authorization;
use super::{planner, RationalizeArgs};
use crate::cli::CmdError;

use audit::build_report;
use human::print_human;
use plan::compile_plan;

struct AuditArgs {
    min_age: u64,
}

#[derive(Debug, Clone, Serialize)]
struct Finding {
    id: String,
    severity: &'static str,
    action: &'static str,
    confidence: &'static str,
    provider: String,
    resource_type: &'static str,
    resource: String,
    reason: String,
    evidence: Value,
    automatic: bool,
}

#[derive(Debug, Clone, Serialize)]
struct SourceReport {
    name: String,
    state: &'static str,
    detail: Value,
}

#[derive(Debug, Clone, Serialize)]
struct ConfigurationSnapshot {
    active_compute: Vec<String>,
    disabled_compute: Vec<String>,
    primary_storage: String,
    backup_storage: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct Summary {
    state: &'static str,
    findings: usize,
    incomplete_sources: usize,
    by_severity: BTreeMap<String, usize>,
    by_action: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, Serialize)]
struct RationalizationReport {
    schema_version: u8,
    generated_at: String,
    read_only: bool,
    min_age_seconds: u64,
    configuration: ConfigurationSnapshot,
    summary: Summary,
    sources: Vec<SourceReport>,
    findings: Vec<Finding>,
}

pub async fn run(args: &RationalizeArgs) -> Result<(), CmdError> {
    let age = planner::parse_age(&args.min_age)?;
    let min_age = u64::try_from(age.num_seconds())
        .map_err(|_| CmdError::usage("--min-age is outside the supported range"))?;
    let report = build_report(&AuditArgs { min_age }).await?;
    let plan = compile_plan(&report, args.provider.as_deref())?;
    let hash = planner::write_plan(&plan, &args.output)?;
    Journal::open().await?.create(&plan).await?;
    if !args.json {
        print_human(&report);
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "operation_id": plan.operation_id,
            "plan": args.output,
            "sha256": hash,
            "findings": plan.findings.len(),
            "actions": plan.actions.len(),
            "automatic_actions": plan.actions.iter().filter(|action| action.authorization == Authorization::Automatic).count(),
            "review_required": plan.actions.iter().filter(|action| action.authorization == Authorization::Explicit).count(),
        }))?
    );
    Ok(())
}
