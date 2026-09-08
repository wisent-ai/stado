//! The three answers `stado repair` gives: `list`, `show`, and a run over one
//! declared service.

use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::deploy::service_catalog::{CatalogRepair, CatalogService};

use super::args::{reject_extra, RepairArgs};
use super::catalog::{catalog, declared_service, declared_step, implementation, DECLARATION};
use super::steps::RepairExecution;

fn step_json(step: &CatalogRepair) -> Value {
    json!({
        "name": step.name,
        "summary": step.summary,
        "mutating": step.mutating,
        "proof": step.proof,
    })
}

async fn list(args: &RepairArgs, services: &[CatalogService]) -> Result<(), CmdError> {
    reject_extra(args, "list")?;
    if args.step_argument.is_some() || args.service_argument.is_some() {
        return Err(CmdError::usage(
            "repair list takes no positional service; pass --service <NAME>.",
        ));
    }
    let selected = match args.service.as_deref() {
        Some(name) => vec![declared_service(services, name)?],
        None => services.iter().collect(),
    };
    let report = json!({
        "declaration": DECLARATION,
        "services": selected.iter().map(|service| json!({
            "name": service.name,
            "summary": service.summary,
            "repair": service.repair.iter().map(step_json).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    });
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        for service in selected {
            println!("{}", service.name);
            if service.repair.is_empty() {
                println!("  no declared repair steps");
            }
            for step in &service.repair {
                let mode = if step.mutating {
                    "mutating"
                } else {
                    "read-only"
                };
                println!("  {:<20} {mode} — {}", step.name, step.summary);
                println!("  {:<20} proof: {}", "", step.proof);
            }
        }
    }
    Ok(())
}

async fn show(args: &RepairArgs, services: &[CatalogService]) -> Result<(), CmdError> {
    reject_extra(args, "show")?;
    if args.service.is_some() {
        return Err(CmdError::usage(
            "repair show takes SERVICE and STEP as positionals, not --service.",
        ));
    }
    let service_name = args
        .service_argument
        .as_deref()
        .ok_or_else(|| CmdError::usage("repair show requires SERVICE and STEP."))?;
    let step_name = args
        .step_argument
        .as_deref()
        .ok_or_else(|| CmdError::usage("repair show requires SERVICE and STEP."))?;
    let service = declared_service(services, service_name)?;
    let step = declared_step(service, step_name)?;
    implementation(&service.name, &step.name)?;
    let report = json!({
        "declaration": DECLARATION,
        "service": service.name,
        "step": step_json(step),
    });
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("{} {}", service.name, step.name);
        println!("{}", step.summary);
        println!(
            "mode: {}",
            if step.mutating {
                "mutating"
            } else {
                "read-only"
            }
        );
        println!("proof: {}", step.proof);
    }
    Ok(())
}

async fn observe_target(target: &str) -> Value {
    match crate::deploy::host_inventory::inventory_host(target, &crate::deploy::production_runner())
        .await
    {
        Ok(report) => report,
        Err(error) => json!({
            "target": target,
            "status": "unavailable",
            "detail": error.to_string(),
        }),
    }
}
fn proof_refusal(service: &str, step: &str, target: &str, proof: &Value) -> Option<String> {
    match (service, step) {
        ("stado", "host")
            if proof.get("status").and_then(Value::as_str)
                != Some(crate::deploy::host_recovery::STATUS_OK) =>
        {
            Some(format!(
                "{target} did not complete host repair; inspect the reported blockers and retry the declared stado host step."
            ))
        }
        ("stado-control-plane", "release-state")
            if proof.get("healthy").and_then(Value::as_bool) != Some(true) =>
        {
            Some(format!(
                "{target} did not complete release-state repair; inspect the reported host drift and failed deliveries."
            ))
        }
        _ => None,
    }
}

async fn run(args: &RepairArgs, services: &[CatalogService]) -> Result<(), CmdError> {
    if args.service_argument.is_some() || args.step_argument.is_some() || args.service.is_some() {
        return Err(CmdError::usage(
            "repair SERVICE accepts --step, --target, --apply, and --json.",
        ));
    }
    let service = declared_service(services, &args.command_or_service)?;
    let steps = match args.step.as_deref() {
        Some(name) => vec![declared_step(service, name)?],
        None => service.repair.iter().collect::<Vec<_>>(),
    };
    if steps.is_empty() {
        return Err(CmdError::click(format!(
            "{} declares no repair steps; add them to {DECLARATION}.",
            service.name
        )));
    }
    for step in &steps {
        implementation(&service.name, &step.name)?;
    }

    let mut reports = Vec::with_capacity(steps.len());
    let mut refusal = None;
    if args.apply {
        let target = args.target.as_deref().ok_or_else(|| {
            CmdError::click(format!(
                "{} declares mutating repair steps but no target was selected; pass --target <TARGET>.",
                service.name
            ))
        })?;
        for step in steps {
            let executable = implementation(&service.name, &step.name)?;
            let execution = RepairExecution {
                service: &service.name,
                target,
            };
            let proof = (executable.function)(&execution).await?;
            let step_refusal = proof_refusal(&service.name, &step.name, target, &proof);
            reports.push(json!({
                "name": step.name,
                "summary": step.summary,
                "mutating": step.mutating,
                "proof": step.proof,
                "status": if step_refusal.is_some() { "incomplete" } else { "applied" },
                "observation": proof,
            }));
            if step_refusal.is_some() {
                refusal = step_refusal;
                break;
            }
        }
    } else {
        let observation = match args.target.as_deref() {
            Some(target) => observe_target(target).await,
            None => json!({
                "status": "target_not_selected",
                "detail": "No target was selected; pass --target <TARGET> to include a read-only host inventory observation."
            }),
        };
        for step in steps {
            reports.push(json!({
                "name": step.name,
                "summary": step.summary,
                "mutating": step.mutating,
                "proof": step.proof,
                "status": "planned",
                "observation": observation,
            }));
        }
    }

    let report = json!({
        "declaration": DECLARATION,
        "service": service.name,
        "target": args.target,
        "applied": args.apply,
        "steps": reports,
    });
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{} repair {}",
            service.name,
            if args.apply { "applied" } else { "dry run" }
        );
        if let Some(target) = &args.target {
            println!("target: {target}");
        }
        for step in report["steps"].as_array().into_iter().flatten() {
            println!(
                "{}: {} — proof: {}",
                step["name"].as_str().unwrap_or("repair"),
                step["status"].as_str().unwrap_or("reported"),
                step["proof"].as_str().unwrap_or("not declared")
            );
            println!("  {}", step["observation"]);
        }
    }
    if let Some(detail) = refusal {
        return Err(CmdError::click(detail));
    }
    Ok(())
}

pub(crate) async fn dispatch(args: RepairArgs) -> Result<(), CmdError> {
    let services = catalog()?;
    match args.command_or_service.as_str() {
        "list" => list(&args, &services).await,
        "show" => show(&args, &services).await,
        _ => run(&args, &services).await,
    }
}
