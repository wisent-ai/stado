//! The executable repair steps and the table that binds them to declaration
//! identities.

use futures::future::BoxFuture;
use serde_json::Value;

use crate::cli::{host, CmdError};

pub(super) struct RepairExecution<'a> {
    pub(super) service: &'a str,
    pub(super) target: &'a str,
}

pub(super) type RepairFunction =
    for<'a> fn(&'a RepairExecution<'a>) -> BoxFuture<'a, Result<Value, CmdError>>;

/// The executable half of one catalog declaration. Both halves are checked as
/// sets before any command answers, so neither an undeclared implementation nor
/// a declaration without code can be silently skipped.
pub(crate) struct RepairStep {
    pub(super) service: &'static str,
    pub(super) name: &'static str,
    pub(super) function: RepairFunction,
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

pub(super) fn implementation_visible(step: &RepairStep) -> bool {
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
