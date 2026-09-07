//! Declared service repair: one capability over the repair steps compiled into
//! the shipped service catalog.

use std::collections::BTreeSet;

use clap::Args;
use futures::future::BoxFuture;
use serde_json::{json, Value};

use super::{host, CmdError};
use crate::deploy::service_catalog::{CatalogRepair, CatalogService};

const DECLARATION: &str = "stado-rs/data/service-catalog.json";

/// `stado repair list`, `stado repair show SERVICE STEP`, or
/// `stado repair SERVICE`. The first positional is deliberately not a clap
/// subcommand: service names are declaration data, so a new service never adds
/// an enum variant or another command.
#[derive(Debug, Args)]
pub(crate) struct RepairArgs {
    /// `list`, `show`, or the declared service name to repair.
    #[arg(value_name = "COMMAND_OR_SERVICE")]
    command_or_service: String,
    /// SERVICE after `show`.
    #[arg(value_name = "SERVICE")]
    service_argument: Option<String>,
    /// STEP after `show`.
    #[arg(value_name = "STEP")]
    step_argument: Option<String>,
    /// Limit `list` to one declared service.
    #[arg(long, value_name = "NAME")]
    service: Option<String>,
    /// Run only this declared step.
    #[arg(long, value_name = "STEP")]
    step: Option<String>,
    /// Registry host on which to observe or apply the repair.
    #[arg(long, value_name = "TARGET")]
    target: Option<String>,
    /// Apply the declared repair; omission is a read-only report.
    #[arg(long)]
    apply: bool,
    /// Emit one machine-readable report.
    #[arg(long)]
    json: bool,
}

struct RepairExecution<'a> {
    service: &'a str,
    target: &'a str,
}

type RepairFunction = for<'a> fn(&'a RepairExecution<'a>) -> BoxFuture<'a, Result<Value, CmdError>>;

/// The executable half of one catalog declaration. Both halves are checked as
/// sets before any command answers, so neither an undeclared implementation nor
/// a declaration without code can be silently skipped.
pub(crate) struct RepairStep {
    service: &'static str,
    name: &'static str,
    function: RepairFunction,
}

fn host_repair<'a>(execution: &'a RepairExecution<'a>) -> BoxFuture<'a, Result<Value, CmdError>> {
    Box::pin(host::apply_host_repair(execution.target))
}

fn object_api<'a>(execution: &'a RepairExecution<'a>) -> BoxFuture<'a, Result<Value, CmdError>> {
    Box::pin(host::apply_object_api_repair(execution.target))
}

fn release_store<'a>(execution: &'a RepairExecution<'a>) -> BoxFuture<'a, Result<Value, CmdError>> {
    Box::pin(host::apply_release_store_repair(
        execution.target,
        execution.service,
    ))
}

fn link<'a>(execution: &'a RepairExecution<'a>) -> BoxFuture<'a, Result<Value, CmdError>> {
    Box::pin(host::apply_link_repair(execution.target))
}

fn release_state<'a>(execution: &'a RepairExecution<'a>) -> BoxFuture<'a, Result<Value, CmdError>> {
    Box::pin(host::apply_release_state_repair(execution.target))
}

fn object_verifier<'a>(
    execution: &'a RepairExecution<'a>,
) -> BoxFuture<'a, Result<Value, CmdError>> {
    Box::pin(host::apply_object_verifier_repair(execution.target))
}

fn release_verifier<'a>(
    execution: &'a RepairExecution<'a>,
) -> BoxFuture<'a, Result<Value, CmdError>> {
    Box::pin(host::apply_release_verifier_repair(execution.target))
}

fn service_verifier<'a>(
    execution: &'a RepairExecution<'a>,
) -> BoxFuture<'a, Result<Value, CmdError>> {
    Box::pin(host::apply_service_verifier_repair(execution.target))
}

fn skarbiec_audit<'a>(
    execution: &'a RepairExecution<'a>,
) -> BoxFuture<'a, Result<Value, CmdError>> {
    Box::pin(host::apply_skarbiec_audit_repair(execution.target))
}

fn skarbiec_crypto<'a>(
    execution: &'a RepairExecution<'a>,
) -> BoxFuture<'a, Result<Value, CmdError>> {
    Box::pin(host::apply_skarbiec_crypto_repair(execution.target))
}

fn skarbiec_acquisition<'a>(
    execution: &'a RepairExecution<'a>,
) -> BoxFuture<'a, Result<Value, CmdError>> {
    Box::pin(host::apply_skarbiec_acquisition_repair(execution.target))
}

fn agent_skarbiec<'a>(
    execution: &'a RepairExecution<'a>,
) -> BoxFuture<'a, Result<Value, CmdError>> {
    Box::pin(host::apply_agent_skarbiec_repair(execution.target))
}

fn storage_root<'a>(execution: &'a RepairExecution<'a>) -> BoxFuture<'a, Result<Value, CmdError>> {
    Box::pin(async move {
        let transaction = format!(
            "repair-{}-{}",
            chrono::Utc::now().format("%Y%m%dT%H%M%SZ"),
            uuid::Uuid::new_v4().simple()
        );
        let accepted = host::storage_root_reconcile_result(execution.target, &transaction, "run")
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
        accepted.outcome?;

        // The resident worker owns the long operation. Read its durable status
        // until it has written the proof receipt rather than treating process
        // launch as proof that storage was reconciled.
        for _ in 0..180 {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let status =
                host::storage_root_reconcile_result(execution.target, &transaction, "status")
                    .await
                    .map_err(|error| CmdError::click(error.to_string()))?;
            status.outcome?;
            match status
                .report
                .pointer("/operation_owner/status")
                .and_then(Value::as_str)
            {
                Some("succeeded") => return Ok(status.report),
                Some("executing") => continue,
                Some(state) => {
                    let detail = status
                        .report
                        .pointer("/operation_owner/error")
                        .and_then(Value::as_str)
                        .unwrap_or("the resident worker supplied no failure detail")
                        .trim_end_matches('.');
                    return Err(CmdError::click(format!(
                        "{} storage-root repair ended {state}; {detail}.",
                        execution.target
                    )));
                }
                None => continue,
            }
        }
        Err(CmdError::click(format!(
            "{} storage-root repair produced no durable completion proof within 360 seconds; inspect transaction {transaction}.",
            execution.target
        )))
    })
}

/// Every executable repair, keyed by its declaration identity.
pub(crate) static REPAIR_STEPS: &[RepairStep] = &[
    RepairStep {
        service: "stado",
        name: "host",
        function: host_repair,
    },
    RepairStep {
        service: "stado",
        name: "object-api",
        function: object_api,
    },
    RepairStep {
        service: "stado",
        name: "release-store",
        function: release_store,
    },
    RepairStep {
        service: "stado",
        name: "link",
        function: link,
    },
    RepairStep {
        service: "stado",
        name: "object-verifier",
        function: object_verifier,
    },
    RepairStep {
        service: "stado",
        name: "release-verifier",
        function: release_verifier,
    },
    RepairStep {
        service: "stado",
        name: "service-verifier",
        function: service_verifier,
    },
    RepairStep {
        service: "stado",
        name: "storage-root",
        function: storage_root,
    },
    RepairStep {
        service: "stado-control-plane",
        name: "release-state",
        function: release_state,
    },
    RepairStep {
        service: "stado-control-plane",
        name: "agent-skarbiec",
        function: agent_skarbiec,
    },
    RepairStep {
        service: "skarbiec",
        name: "audit-lock",
        function: skarbiec_audit,
    },
    RepairStep {
        service: "skarbiec",
        name: "crypto",
        function: skarbiec_crypto,
    },
    RepairStep {
        service: "skarbiec",
        name: "acquisition-state",
        function: skarbiec_acquisition,
    },
];

fn implementation_visible(step: &RepairStep) -> bool {
    // Integration tests must prove the runtime mismatch refusal through the
    // real binary. Debug builds may hide one implementation from validation;
    // release binaries have no declaration override or implementation switch.
    #[cfg(debug_assertions)]
    {
        if let Ok(hidden) = std::env::var("STADO_REPAIR_TEST_MISSING_IMPLEMENTATION") {
            if hidden.split_once(':') == Some((step.service, step.name)) {
                return false;
            }
        }
    }
    true
}

fn catalog() -> Result<Vec<CatalogService>, CmdError> {
    let services = crate::deploy::service_catalog::all().map_err(CmdError::click)?;
    validate(&services)?;
    Ok(services)
}

fn validate(services: &[CatalogService]) -> Result<(), CmdError> {
    let mut declarations = BTreeSet::new();
    for service in services {
        for step in &service.repair {
            let key = (service.name.as_str(), step.name.as_str());
            if !declarations.insert(key) {
                return Err(CmdError::click(format!(
                    "{} declares repair step {} more than once; keep one row in {DECLARATION}.",
                    service.name, step.name
                )));
            }
            if !REPAIR_STEPS.iter().any(|implementation| {
                implementation_visible(implementation)
                    && implementation.service == service.name
                    && implementation.name == step.name
            }) {
                return Err(CmdError::click(format!(
                    "{} repair step {} declares no implementation; add it to stado-rs/src/cli/repair.rs.",
                    service.name, step.name
                )));
            }
        }
    }

    let mut implementations = BTreeSet::new();
    for implementation in REPAIR_STEPS
        .iter()
        .filter(|implementation| implementation_visible(implementation))
    {
        if !implementations.insert((implementation.service, implementation.name)) {
            return Err(CmdError::click(format!(
                "{} repair step {} has more than one implementation; keep one entry in stado-rs/src/cli/repair.rs.",
                implementation.service, implementation.name
            )));
        }
        if !declarations.contains(&(implementation.service, implementation.name)) {
            return Err(CmdError::click(format!(
                "{} implements repair step {} but declares no repair; add it to {DECLARATION}.",
                implementation.service, implementation.name
            )));
        }
    }
    Ok(())
}

fn declared_service<'a>(
    services: &'a [CatalogService],
    name: &str,
) -> Result<&'a CatalogService, CmdError> {
    services
        .iter()
        .find(|service| service.name == name || service.unit.as_deref() == Some(name))
        .ok_or_else(|| {
            CmdError::click(format!(
                "{name} declares no repair; add it to {DECLARATION}."
            ))
        })
}

fn declared_step<'a>(
    service: &'a CatalogService,
    name: &str,
) -> Result<&'a CatalogRepair, CmdError> {
    service
        .repair
        .iter()
        .find(|step| step.name == name)
        .ok_or_else(|| {
            CmdError::click(format!(
                "{} declares no repair step {name}; add it to {DECLARATION}.",
                service.name
            ))
        })
}

fn implementation(service: &str, name: &str) -> Result<&'static RepairStep, CmdError> {
    REPAIR_STEPS
        .iter()
        .find(|step| {
            implementation_visible(step) && step.service == service && step.name == name
        })
        .ok_or_else(|| {
            CmdError::click(format!(
                "{service} repair step {name} declares no implementation; add it to stado-rs/src/cli/repair.rs."
            ))
        })
}

fn reject_extra(args: &RepairArgs, operation: &str) -> Result<(), CmdError> {
    if args.step.is_some() || args.target.is_some() || args.apply {
        return Err(CmdError::usage(format!(
            "repair {operation} accepts only its documented declaration filters and --json."
        )));
    }
    Ok(())
}

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
