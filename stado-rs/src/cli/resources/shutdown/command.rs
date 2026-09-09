//! The `resources shutdown` entry point: prove the selection, draft one
//! action per resource, then write and journal the plan.

use serde_json::json;
use sha2::{Digest, Sha256};

use crate::cli::resources::executors::Context;
use crate::cli::resources::journal::Journal;
use crate::cli::resources::model::{
    Intent, InventorySnapshot, OperationScope, ProviderKind, SourceSnapshot,
};
use crate::cli::resources::{planner, ShutdownArgs};
use crate::cli::CmdError;
use crate::queue::copy::Endpoint;

use super::discover::discover_owned;
use super::finalize::finalize;
use super::selector::{parse_selector, validate_project};

pub async fn run(args: &ShutdownArgs) -> Result<(), CmdError> {
    validate_project(&args.project)?;
    if !args.all_stado_owned && args.resource.is_empty() {
        return Err(CmdError::usage(
            "shutdown requires --all-stado-owned or at least one --resource",
        ));
    }
    let (selectors, inventory) = if args.all_stado_owned {
        discover_owned(args).await?
    } else {
        (
            args.resource.clone(),
            InventorySnapshot {
                snapshot_id: hex::encode(Sha256::digest(args.resource.join("\n").as_bytes())),
                complete: true,
                sources: vec![SourceSnapshot {
                    name: "operator-selectors".to_string(),
                    state: "ok".to_string(),
                    detail: json!({"resources": args.resource}),
                }],
            },
        )
    };
    let mut drafts = Vec::new();
    for selector in selectors {
        drafts.push(parse_selector(&args.project, &selector)?);
    }
    if drafts.is_empty() {
        return Err(CmdError::click(
            "shutdown discovery found no authoritative Stado-owned resources",
        ));
    }
    let context = Context::new(&drafts).await?;
    let mut actions = Vec::new();
    for draft in drafts {
        let observed = context.inspect(&draft).await?;
        if let Some(action) = finalize(draft, observed)? {
            actions.push(action);
        }
    }
    let plan = planner::new_plan(
        Intent::Shutdown,
        OperationScope {
            providers: [ProviderKind::Gcp].into_iter().collect(),
            projects: [args.project.clone()].into_iter().collect(),
            storage: Endpoint::configured_primary().describe(),
        },
        inventory,
        Vec::new(),
        actions,
    )?;
    let hash = planner::write_plan(&plan, &args.output)?;
    Journal::open().await?.create(&plan).await?;
    let summary = json!({
        "operation_id": plan.operation_id,
        "plan": args.output,
        "sha256": hash,
        "actions": plan.actions.len(),
        "reversible": plan.actions.iter().all(|action| action.rollback.is_some()),
        "safety": "no delete actions and no billing mutation",
    });
    if args.json {
        println!("{}", serde_json::to_string_pretty(&summary)?);
    } else {
        println!(
            "shutdown plan {}: {} reversible action(s); SHA-256 {}",
            plan.operation_id,
            plan.actions.len(),
            hash
        );
        println!("written to {}", args.output.display());
    }
    Ok(())
}
